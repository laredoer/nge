// 简化 PBR：Cook-Torrance（GGX + Smith + Schlick），环境光 = albedo × ambient，不做 IBL 和色调映射。

struct DirLight { to_light: vec4f, color: vec4f }
struct PointLight { position_range: vec4f, color: vec4f }
struct Globals {
    view_proj: mat4x4f,
    eye: vec4f,
    ambient: vec4f,
    counts: vec4u,
    dirs: array<DirLight, 4>,
    points: array<PointLight, 8>,
    light_view_proj: mat4x4f,
    // 法线方向往外推多少、阴影图一个像素在 uv 里多大
    shadow: vec4f,
}
struct Draw {
    model: mat4x4f,
    normal: mat4x4f,
    base_color: vec4f,
    // metallic, roughness, alpha_cutoff（< 0 表示不透明）, 镜像（> 0）
    params: vec4f,
    emissive: vec4f,
    // x：关节矩阵从第几个开始（蒙皮的）
    skin: vec4u,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> d: Draw;
@group(1) @binding(1) var<storage, read> joints: array<mat4x4f>;
@group(2) @binding(0) var base_tex: texture_2d<f32>;
@group(2) @binding(1) var base_sampler: sampler;

struct VsIn {
    @location(0) position: vec3f,
    @location(1) normal: vec3f,
    @location(2) uv: vec2f,
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

@vertex
fn vs(v: VsIn) -> VsOut {
    return vertex(vec4f(v.position, 1.0), vec4f(v.normal, 0.0), v.uv);
}

/// 关节矩阵按权重混合；没有权重的顶点不动。法线也用它变换（关节大多只转不拉伸，够用）。
@vertex
fn vs_skinned(v: VsIn, s: SkinIn) -> VsOut {
    let o = d.skin.x;
    let j = s.joints;
    let w = s.weights;
    var m = joints[o + j.x] * w.x + joints[o + j.y] * w.y + joints[o + j.z] * w.z + joints[o + j.w] * w.w;
    if dot(w, vec4f(1.0)) <= 0.0 {
        m = mat4x4f(vec4f(1.0, 0.0, 0.0, 0.0), vec4f(0.0, 1.0, 0.0, 0.0), vec4f(0.0, 0.0, 1.0, 0.0), vec4f(0.0, 0.0, 0.0, 1.0));
    }
    return vertex(m * vec4f(v.position, 1.0), m * vec4f(v.normal, 0.0), v.uv);
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

/// 这一点被投影子的光照到多少（0 全挡、1 全照到）：3×3 PCF，阴影图外面算照到。
fn lit(world: vec3f, n: vec3f) -> f32 {
    let p = g.light_view_proj * vec4f(world + n * g.shadow.x, 1.0);
    let uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) || p.z > 1.0 {
        return 1.0;
    }
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            sum += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2f(f32(dx), f32(dy)) * g.shadow.y, p.z);
        }
    }
    return sum / 9.0;
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
            radiance *= lit(i.world, n);
        }
        color += shade(n, v, light.to_light.xyz, radiance, base.rgb, metallic, roughness);
    }
    for (var k = 0u; k < g.counts.y; k++) {
        let light = g.points[k];
        let to = light.position_range.xyz - i.world;
        let dist2 = max(dot(to, to), 1e-4);
        let falloff = clamp(1.0 - pow(dist2 / (light.position_range.w * light.position_range.w), 2.0), 0.0, 1.0);
        let radiance = light.color.rgb * falloff * falloff / dist2;
        color += shade(n, v, to * inverseSqrt(dist2), radiance, base.rgb, metallic, roughness);
    }
    return vec4f(color, 1.0);
}
