// 简化 PBR：Cook-Torrance（GGX + Smith + Schlick），环境光 = albedo × ambient，不做 IBL 和色调映射。

struct DirLight { to_light: vec4f, color: vec4f }
struct PointLight { position_range: vec4f, color: vec4f }
// direction.w 是外锥的 cos，color.w 是内锥的 cos
struct SpotLight { position_range: vec4f, direction: vec4f, color: vec4f }
struct Globals {
    view_proj: mat4x4f,
    eye: vec4f,
    ambient: vec4f,
    // 方向光数量、点光源数量、投影子的方向光（第几盏 + 1）、聚光灯数量
    counts: vec4u,
    // 投影子的聚光灯、点光源（第几盏 + 1），方向光阴影的段数
    casters: vec4u,
    dirs: array<DirLight, 4>,
    points: array<PointLight, 8>,
    spots: array<SpotLight, 4>,
    cascades: array<mat4x4f, 4>,
    // 0 聚光灯，1–6 点光源 +X、-X、+Y、-Y、+Z、-Z
    locals: array<mat4x4f, 7>,
    // 片元在相机前方的距离 = dot(xyz, 位置) + w
    view_forward: vec4f,
    // 每段的远端
    splits: vec4f,
    // 每段法线方向往外推多少
    cascade_offset: vec4f,
    // 方向光、局部光的图一个像素在 uv 里多大；聚光灯、点光源离灯 1 处往外推多少
    shadow: vec4f,
}
struct Draw {
    model: mat4x4f,
    normal: mat4x4f,
    base_color: vec4f,
    // metallic, roughness, alpha_cutoff（< 0 表示不透明）, 镜像（> 0）
    params: vec4f,
    // w > 0：半透明
    emissive: vec4f,
    // x：关节矩阵从第几个开始（蒙皮的）
    skin: vec4u,
    // x：变形差从第几个 vec4 开始，y：目标数，z：顶点数
    morph: vec4u,
    weights: array<vec4f, 2>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(0) @binding(3) var local_map: texture_depth_2d_array;
@group(1) @binding(0) var<uniform> d: Draw;
@group(1) @binding(1) var<storage, read> joints: array<mat4x4f>;
// 每个目标每个顶点两个：位置差、法线差
@group(1) @binding(2) var<storage, read> morphs: array<vec4f>;
@group(2) @binding(0) var base_tex: texture_2d<f32>;
@group(2) @binding(1) var base_sampler: sampler;

struct VsIn {
    @location(0) position: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
    @builtin(vertex_index) index: u32,
}

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(0) world: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
}

struct SkinIn {
    @location(3) joints: vec4u,
    @location(4) weights: vec4f,
}

fn vertex(position: vec4f, normal: vec4f, uv: vec2f) -> VsOut {
    let world = d.model * position;
    var o: VsOut;
    o.clip = g.view_proj * world;
    o.world = world.xyz;
    o.normal = (d.normal * normal).xyz;
    o.uv = uv;
    return o;
}

struct Morphed {
    position: vec3f,
    normal: vec3f,
}

/// 按权重加上每个变形目标的位置差、法线差。
fn morph(index: u32, position: vec3f, normal: vec3f) -> Morphed {
    var o = Morphed(position, normal);
    for (var t = 0u; t < d.morph.y; t++) {
        let w = d.weights[t / 4u][t % 4u];
        let k = d.morph.x + (t * d.morph.z + index) * 2u;
        o.position += morphs[k].xyz * w;
        o.normal += morphs[k + 1u].xyz * w;
    }
    return o;
}

@vertex
fn vs(v: VsIn) -> VsOut {
    let p = morph(v.index, v.position, v.normal);
    return vertex(vec4f(p.position, 1.0), vec4f(p.normal, 0.0), v.uv);
}

/// 先变形，再按权重混合的关节矩阵蒙皮；没有权重的顶点不动。法线也用它变换（关节大多只转不拉伸，够用）。
@vertex
fn vs_skinned(v: VsIn, s: SkinIn) -> VsOut {
    let o = d.skin.x;
    let j = s.joints;
    let w = s.weights;
    var m = joints[o + j.x] * w.x + joints[o + j.y] * w.y + joints[o + j.z] * w.z + joints[o + j.w] * w.w;
    if dot(w, vec4f(1.0)) <= 0.0 {
        m = mat4x4f(vec4f(1.0, 0.0, 0.0, 0.0), vec4f(0.0, 1.0, 0.0, 0.0), vec4f(0.0, 0.0, 1.0, 0.0), vec4f(0.0, 0.0, 0.0, 1.0));
    }
    let p = morph(v.index, v.position, v.normal);
    return vertex(m * vec4f(p.position, 1.0), m * vec4f(p.normal, 0.0), v.uv);
}

const PI: f32 = 3.14159265;

fn shade(n: vec3f, v: vec3f, l: vec3f, radiance: vec3f, albedo: vec3f, metallic: f32, roughness: f32) -> vec3f {
    let n_l = dot(n, l);
    if n_l <= 0.0 {
        return vec3f(0.0);
    }
    let h = normalize(v + l);
    let n_v = max(dot(n, v), 1e-4);
    let n_h = max(dot(n, h), 0.0);
    let v_h = max(dot(v, h), 0.0);
    let a2 = pow(roughness, 4.0);
    let dd = n_h * n_h * (a2 - 1.0) + 1.0;
    let ndf = a2 / (PI * dd * dd);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let geo = (n_v / (n_v * (1.0 - k) + k)) * (n_l / (n_l * (1.0 - k) + k));
    let f0 = mix(vec3f(0.04), albedo, metallic);
    let fresnel = f0 + (1.0 - f0) * pow(clamp(1.0 - v_h, 0.0, 1.0), 5.0);
    let specular = ndf * geo * fresnel / (4.0 * n_v * n_l);
    let diffuse = (1.0 - fresnel) * (1.0 - metallic) * albedo / PI;
    return (diffuse + specular) * radiance * n_l;
}

/// 阴影图第 `layer` 层在裁剪坐标 `c` 处被照到多少（0 全挡、1 全照到）：3×3 PCF，图外面、灯背后算照到。
fn pcf(map: texture_depth_2d_array, layer: u32, c: vec4f, texel: f32) -> f32 {
    if c.w <= 0.0 {
        return 1.0;
    }
    let p = c.xyz / c.w;
    let uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) || p.z > 1.0 {
        return 1.0;
    }
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            sum += textureSampleCompareLevel(map, shadow_sampler, uv + vec2f(f32(dx), f32(dy)) * texel, layer, p.z);
        }
    }
    return sum / 9.0;
}

/// 方向光：按片元在相机前方的距离挑段，最后一段以外没有影子。
fn lit_directional(world: vec3f, n: vec3f) -> f32 {
    let depth = dot(world, g.view_forward.xyz) + g.view_forward.w;
    for (var c = 0u; c < g.casters.z; c++) {
        if depth < g.splits[c] {
            return pcf(shadow_map, c, g.cascades[c] * vec4f(world + n * g.cascade_offset[c], 1.0), g.shadow.x);
        }
    }
    return 1.0;
}

/// 聚光灯、点光源：一个像素在世界里多大按沿视线到灯的距离放大。
fn lit_local(layer: u32, world: vec3f, n: vec3f, depth: f32, per_unit: f32) -> f32 {
    return pcf(local_map, layer, g.locals[layer] * vec4f(world + n * depth * per_unit, 1.0), g.shadow.y);
}

/// 点光源：推出去的量按片元到灯最远的那一轴算；挑面用推出去之后的点（绝对值最大的轴），和实际采样的点一致，
/// 不然面和面交界的对角线附近推出去的点落到挑中的面外面，算成照亮。
fn lit_point(world: vec3f, n: vec3f, light: vec3f) -> f32 {
    let a0 = abs(world - light);
    let p = world + n * max(a0.x, max(a0.y, a0.z)) * g.shadow.w;
    let v = p - light;
    let a = abs(v);
    var face = 4u + select(0u, 1u, v.z < 0.0);
    if a.x >= a.y && a.x >= a.z {
        face = select(0u, 1u, v.x < 0.0);
    } else if a.y >= a.z {
        face = 2u + select(0u, 1u, v.y < 0.0);
    }
    return pcf(local_map, 1u + face, g.locals[1u + face] * vec4f(p, 1.0), g.shadow.y);
}

/// 距离衰减：按距离平方，到 range 平滑降到 0。
fn attenuation(dist2: f32, range: f32) -> f32 {
    let falloff = clamp(1.0 - pow(dist2 / (range * range), 2.0), 0.0, 1.0);
    return falloff * falloff / dist2;
}

@fragment
fn fs(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let base = textureSample(base_tex, base_sampler, i.uv) * d.base_color;
    if d.params.z >= 0.0 && base.a < d.params.z {
        discard;
    }
    var n = normalize(i.normal);
    // params.w > 0：镜像变换，光栅化的正反面和几何上的相反
    if front == (d.params.w > 0.0) {
        n = -n;
    }
    // eye.w = 0：正交相机，所有片元的视线方向一样
    let v = normalize(select(g.eye.xyz - i.world, g.eye.xyz, g.eye.w == 0.0));
    let metallic = clamp(d.params.x, 0.0, 1.0);
    let roughness = clamp(d.params.y, 0.04, 1.0);

    var color = g.ambient.rgb * base.rgb + d.emissive.rgb;
    for (var k = 0u; k < g.counts.x; k++) {
        let light = g.dirs[k];
        var radiance = light.color.rgb;
        if g.counts.z == k + 1u {
            radiance *= lit_directional(i.world, n);
        }
        color += shade(n, v, light.to_light.xyz, radiance, base.rgb, metallic, roughness);
    }
    for (var k = 0u; k < g.counts.y; k++) {
        let light = g.points[k];
        let to = light.position_range.xyz - i.world;
        let dist2 = max(dot(to, to), 1e-4);
        var radiance = light.color.rgb * attenuation(dist2, light.position_range.w);
        if g.casters.y == k + 1u {
            radiance *= lit_point(i.world, n, light.position_range.xyz);
        }
        color += shade(n, v, to * inverseSqrt(dist2), radiance, base.rgb, metallic, roughness);
    }
    for (var k = 0u; k < g.counts.w; k++) {
        let light = g.spots[k];
        let to = light.position_range.xyz - i.world;
        let dist2 = max(dot(to, to), 1e-4);
        let l = to * inverseSqrt(dist2);
        // 锥的系数：smoothstep(cos 外, cos 内, cos θ)；内外锥一样时是硬边
        let t = clamp((dot(-l, light.direction.xyz) - light.direction.w) / max(light.color.w - light.direction.w, 1e-4), 0.0, 1.0);
        var radiance = light.color.rgb * attenuation(dist2, light.position_range.w) * t * t * (3.0 - 2.0 * t);
        if g.casters.x == k + 1u {
            radiance *= lit_local(0u, i.world, n, dot(-to, light.direction.xyz), g.shadow.z);
        }
        color += shade(n, v, l, radiance, base.rgb, metallic, roughness);
    }
    return vec4f(color, select(1.0, base.a, d.emissive.w > 0.0));
}
