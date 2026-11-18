// 简化 PBR：Cook-Torrance（GGX + Smith + Schlick）；环境光 = albedo × ambient，有天空时再加 SH9 漫反射和预滤波镜面（split-sum），不做色调映射。
// 前面拼着 fog.wgsl；`POISSON_DISK` 建管线时换成 shadow_soft.rs 算的采样盘。

struct DirLight { to_light: vec4f, color: vec4f }
struct PointLight { position_range: vec4f, color: vec4f }
// direction.w 是外锥的 cos，color.w 是内锥的 cos
struct SpotLight { position_range: vec4f, direction: vec4f, color: vec4f }
struct Globals {
    view_proj: mat4x4f,
    inv_view_proj: mat4x4f,
    eye: vec4f,
    ambient: vec4f,
    // 方向光数量、点光源数量、投影子的方向光（第几盏 + 1）、聚光灯数量
    counts: vec4u,
    // 阴影过滤（0 PCF、1 泊松、2 PCSS）、级联调试色、方向光阴影的段数、画天空（0 不画、1 程序天空、2 立方体）
    casters: vec4u,
    dirs: array<DirLight, 4>,
    points: array<PointLight, 8>,
    spots: array<SpotLight, 4>,
    cascades: array<mat4x4f, 4>,
    // 聚光灯 1 层、点光源 6 层（+X、-X、+Y、-Y、+Z、-Z）
    locals: array<mat4x4f, 24>,
    // 片元在相机前方的距离 = dot(xyz, 位置) + w
    view_forward: vec4f,
    // 每段的远端
    splits: vec4f,
    // 每段法线方向往外推多少
    cascade_offset: vec4f,
    // 方向光、局部光的图一个像素在 uv 里多大；w：点光源离灯 1 处往外推多少
    shadow: vec4f,
    // 每盏聚光灯 / 点光源的阴影从第几层开始 + 1（0 不投影）
    spot_layers: vec4u,
    point_layers: array<vec4u, 2>,
    // 每盏聚光灯离灯 1 处往外推多少
    spot_offset: vec4f,
    // 泊松盘半径（像素）、级联混合比例、深度 bias、PCSS 最大半影（像素）
    soft: vec4f,
    // PCSS：每段深度每差 1 半影宽几个像素
    pcss: vec4f,
    // 环境漫反射、镜面强度、预滤波最高一级、有没有环境光照
    env: vec4f,
    sh: array<vec4f, 9>,
    // 程序天空：上、地平线、下；指向太阳（w 半角）；太阳色（w：1 有太阳）
    sky: array<vec4f, 5>,
    fog: Fog,
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
@group(0) @binding(4) var env_cube: texture_cube<f32>;
@group(0) @binding(5) var sky_cube: texture_cube<f32>;
@group(0) @binding(6) var brdf_lut: texture_2d<f32>;
@group(0) @binding(7) var env_sampler: sampler;
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

var<private> DISK: array<vec2f, 16> = POISSON_DISK;

/// 交错梯度噪声：每个像素一个 0–1 的数，转采样盘用。
/// 采样盘按阴影图像素转的角度（4 × 4 Bayer，0–2π）：跟着阴影图的像素格走，镜头动时纹样不游动，静止时也没有斜纹。
fn rotation(uv: vec2f, size: vec2f) -> f32 {
    let p = vec2u(floor(uv * size)) % 4u;
    var bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    return (bayer[p.y * 4u + p.x] + 0.5) / 16.0 * 6.2831853;
}

/// 裁剪坐标 → 阴影图的 (u, v, 深度)；图外面、灯背后 w < 0（算照到）。
fn shadow_uv(c: vec4f) -> vec4f {
    if c.w <= 0.0 {
        return vec4f(0.0, 0.0, 0.0, -1.0);
    }
    let p = c.xyz / c.w;
    let uv = vec2f(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) || p.z > 1.0 {
        return vec4f(0.0, 0.0, 0.0, -1.0);
    }
    return vec4f(uv, p.z - g.soft.z, 1.0);
}

fn pcf3(map: texture_depth_2d_array, layer: u32, s: vec4f, texel: f32) -> f32 {
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            sum += textureSampleCompareLevel(map, shadow_sampler, s.xy + vec2f(f32(dx), f32(dy)) * texel, layer, s.z);
        }
    }
    return sum / 9.0;
}

/// 采样盘转 `angle`、半径 `radius`（uv）的 16 点 PCF。
fn disk_pcf(map: texture_depth_2d_array, layer: u32, s: vec4f, radius: f32, angle: f32) -> f32 {
    let r = mat2x2f(cos(angle), sin(angle), -sin(angle), cos(angle));
    var sum = 0.0;
    for (var i = 0u; i < 16u; i++) {
        sum += textureSampleCompareLevel(map, shadow_sampler, s.xy + r * DISK[i] * radius, layer, s.z);
    }
    return sum / 16.0;
}

/// 第 `layer` 层在裁剪坐标 `c` 处被照到多少（0 全挡、1 全照到）：PCF 或采样盘。
fn filtered(map: texture_depth_2d_array, layer: u32, c: vec4f, texel: f32) -> f32 {
    let s = shadow_uv(c);
    if s.w < 0.0 {
        return 1.0;
    }
    if g.casters.x == 0u {
        return pcf3(map, layer, s, texel);
    }
    return disk_pcf(map, layer, s, g.soft.x * texel, rotation(s.xy, vec2f(textureDimensions(map))));
}

/// PCSS（方向光）：先在最大半影里找挡光的，平均遮挡深度和片元深度差 × `scale` 是半影宽（直径，像素），按一半做采样盘的半径。
/// 找挡光的时候按片元所在平面（法线 `n`）在光空间里的斜率补偿每个采样点的深度，斜面不会把自己算成挡光的。
/// 每个像素：找挡光的 16 次 `textureLoad`，加上采样盘 16 次比较采样。
fn pcss(layer: u32, c: vec4f, n: vec3f, texel: f32, scale: f32) -> f32 {
    let s = shadow_uv(c);
    if s.w < 0.0 {
        return 1.0;
    }
    let size = vec2f(textureDimensions(shadow_map));
    let angle = rotation(s.xy, size);
    let r = mat2x2f(cos(angle), sin(angle), -sin(angle), cos(angle));
    let slope = receiver_slope(g.cascades[layer], n);
    var sum = 0.0;
    var blockers = 0.0;
    for (var i = 0u; i < 16u; i++) {
        let offset = r * DISK[i] * g.soft.w * texel;
        let uv = s.xy + offset;
        let at = clamp(vec2i(uv * size), vec2i(0), vec2i(size) - 1);
        let d = textureLoad(shadow_map, at, layer, 0);
        if d < s.z + dot(slope, offset) {
            sum += d;
            blockers += 1.0;
        }
    }
    if blockers == 0.0 {
        return 1.0;
    }
    let penumbra = clamp(0.5 * (s.z - sum / blockers) * scale, 1.0, g.soft.w);
    return disk_pcf(shadow_map, layer, s, penumbra * texel, angle);
}

/// 法线为 `n` 的平面在阴影图上的深度斜率（深度 / uv）：取平面上两个方向，看它们在光空间里怎么变。几乎侧对着光时是 0。
fn receiver_slope(m: mat4x4f, n: vec3f) -> vec2f {
    var t = cross(n, vec3f(0.0, 1.0, 0.0));
    if dot(t, t) < 1e-4 {
        t = cross(n, vec3f(1.0, 0.0, 0.0));
    }
    let a = (m * vec4f(normalize(t), 0.0)).xyz;
    let b = (m * vec4f(normalize(cross(n, t)), 0.0)).xyz;
    let au = a.x * 0.5;
    let av = -a.y * 0.5;
    let bu = b.x * 0.5;
    let bv = -b.y * 0.5;
    let det = au * bv - bu * av;
    if abs(det) < 1e-8 {
        return vec2f(0.0);
    }
    return vec2f(a.z * bv - b.z * av, au * b.z - bu * a.z) / det;
}

fn cascade_lit(c: u32, world: vec3f, n: vec3f) -> f32 {
    let clip = g.cascades[c] * vec4f(world + n * g.cascade_offset[c], 1.0);
    if g.casters.x == 2u {
        return pcss(c, clip, n, g.shadow.x, g.pcss[c]);
    }
    return filtered(shadow_map, c, clip, g.shadow.x);
}

/// 片元在相机前方 `depth` 处用第几段（x，-1 是最后一段以外）、往下一段混多少（y）；和 shadow_soft.rs 的 `cascade_blend` 一样。
fn cascade_of(depth: f32) -> vec2f {
    for (var c = 0u; c < g.casters.z; c++) {
        let end = g.splits[c];
        if depth < end {
            if end >= 3.0e38 {
                return vec2f(f32(c), 0.0);
            }
            var start = 0.0;
            if c > 0u {
                start = g.splits[c - 1u];
            }
            let band = g.soft.y * (end - start);
            return vec2f(f32(c), select(0.0, clamp((depth - (end - band)) / band, 0.0, 1.0), band > 0.0));
        }
    }
    return vec2f(-1.0, 0.0);
}

/// 方向光：按片元在相机前方的距离挑段，交界前混下一段，最后一段末尾淡出到照到。
fn lit_directional(world: vec3f, n: vec3f) -> f32 {
    let at = cascade_of(dot(world, g.view_forward.xyz) + g.view_forward.w);
    if at.x < 0.0 {
        return 1.0;
    }
    let c = u32(at.x);
    var lit = cascade_lit(c, world, n);
    if at.y > 0.0 {
        var next = 1.0;
        if c + 1u < g.casters.z {
            next = cascade_lit(c + 1u, world, n);
        }
        lit = mix(lit, next, at.y);
    }
    return lit;
}

/// 级联调试色：红、绿、蓝、黄，交界处按混合权重混。
fn cascade_tint(world: vec3f) -> vec3f {
    var colors = array<vec3f, 4>(vec3f(1.0, 0.35, 0.35), vec3f(0.35, 1.0, 0.35), vec3f(0.35, 0.35, 1.0), vec3f(1.0, 1.0, 0.35));
    let at = cascade_of(dot(world, g.view_forward.xyz) + g.view_forward.w);
    if at.x < 0.0 {
        return vec3f(1.0);
    }
    let c = u32(at.x);
    var next = vec3f(1.0);
    if c + 1u < g.casters.z {
        next = colors[c + 1u];
    }
    return mix(colors[c], next, at.y);
}

/// 聚光灯：一个像素在世界里多大按沿视线到灯的距离放大。
fn lit_local(layer: u32, world: vec3f, n: vec3f, depth: f32, per_unit: f32) -> f32 {
    return filtered(local_map, layer, g.locals[layer] * vec4f(world + n * depth * per_unit, 1.0), g.shadow.y);
}

/// 点光源（六面从 `first` 层开始）：推出去的量按片元到灯最远的那一轴算；挑面用推出去之后的点（绝对值最大的轴），
/// 和实际采样的点一致，不然面和面交界的对角线附近推出去的点落到挑中的面外面，算成照亮。
fn lit_point(first: u32, world: vec3f, n: vec3f, light: vec3f) -> f32 {
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
    return filtered(local_map, first + face, g.locals[first + face] * vec4f(p, 1.0), g.shadow.y);
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
    if g.env.w > 0.0 {
        color += environment(n, v, base.rgb, metallic, roughness);
    }
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
        let layer = g.point_layers[k / 4u][k % 4u];
        if layer > 0u {
            radiance *= lit_point(layer - 1u, i.world, n, light.position_range.xyz);
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
        let layer = g.spot_layers[k];
        if layer > 0u {
            radiance *= lit_local(layer - 1u, i.world, n, dot(-to, light.direction.xyz), g.spot_offset[k]);
        }
        color += shade(n, v, l, radiance, base.rgb, metallic, roughness);
    }
    color = apply_fog(color, i.world, g.fog, g.sh);
    if g.casters.y == 1u && g.counts.z > 0u {
        color *= cascade_tint(i.world);
    }
    return vec4f(color, select(1.0, base.a, d.emissive.w > 0.0));
}

/// 环境光照：SH9 辐照度的漫反射 + 预滤波 mip 链按粗糙度取一级的镜面（split-sum，BRDF LUT 给 F0 的缩放和偏移）。
fn environment(n: vec3f, v: vec3f, albedo: vec3f, metallic: f32, roughness: f32) -> vec3f {
    let n_v = max(dot(n, v), 1e-4);
    let f0 = mix(vec3f(0.04), albedo, metallic);
    let fr = f0 + (max(vec3f(1.0 - roughness), f0) - f0) * pow(1.0 - n_v, 5.0);
    let diffuse = max(sh_irradiance(g.sh, n), vec3f(0.0)) * albedo / PI * (1.0 - fr) * (1.0 - metallic) * g.env.x;
    let prefiltered = textureSampleLevel(env_cube, env_sampler, reflect(-v, n), roughness * g.env.z).rgb;
    let ab = textureSampleLevel(brdf_lut, env_sampler, vec2f(n_v, roughness), 0.0).rg;
    return diffuse + prefiltered * (f0 * ab.x + ab.y) * g.env.y;
}

/// 程序天空，和 sky.rs 的 `SkyGradient::radiance` 是同一个公式。
fn sky_gradient(d: vec3f) -> vec3f {
    var c = mix(g.sky[1].rgb, g.sky[0].rgb, sqrt(clamp(d.y, 0.0, 1.0)));
    if d.y < 0.0 {
        c = mix(g.sky[1].rgb, g.sky[2].rgb, sqrt(clamp(-d.y, 0.0, 1.0)));
    }
    if g.sky[4].w > 0.0 {
        let cos_sun = dot(d, g.sky[3].xyz);
        let size = max(g.sky[3].w, 1e-4);
        let disk = smoothstep(cos(size * 1.2), cos(size), cos_sun);
        let glow = pow(max(cos_sun, 0.0), 64.0) * 0.25;
        c += g.sky[4].rgb * (disk + glow);
    }
    return c;
}

struct SkyOut {
    @builtin(position) clip: vec4f,
    @location(0) ndc: vec2f,
}

/// 盖满屏幕的三角形，画在远平面（深度 1）。
@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyOut {
    let p = vec2f(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    var o: SkyOut;
    o.clip = vec4f(p, 1.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_sky(i: SkyOut) -> @location(0) vec4f {
    let near = g.inv_view_proj * vec4f(i.ndc, 0.0, 1.0);
    let far = g.inv_view_proj * vec4f(i.ndc, 1.0, 1.0);
    let d = normalize(far.xyz / far.w - near.xyz / near.w);
    if g.casters.w == 2u {
        return vec4f(textureSampleLevel(sky_cube, env_sampler, d, 0.0).rgb, 1.0);
    }
    return vec4f(sky_gradient(d), 1.0);
}
