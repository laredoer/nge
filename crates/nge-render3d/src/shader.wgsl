// PBR（Cook-Torrance GGX + Smith + Schlick + 完整贴图 Normal/MR/AO/Emissive）+ 风格化材质（Toon / Water / Foliage）+ GPU 实例化；
// 环境光 = albedo × ambient × ao，有天空时再加 SH9 漫反射和预滤波镜面（split-sum）。
// 前面拼着 fog.wgsl；`POISSON_DISK` 建管线时换成 shadow_soft.rs 算的采样盘。

struct DirLight { to_light: vec4f, color: vec4f }
struct PointLight { position_range: vec4f, color: vec4f }
// direction.w 是外锥的 cos，color.w 是内锥的 cos
struct SpotLight { position_range: vec4f, direction: vec4f, color: vec4f }
struct Globals {
    view_proj: mat4x4f,
    inv_view_proj: mat4x4f,
    eye: vec4f,
    // rgb 环境光，w 经过秒数（水波 / 树叶风动用）
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
    // x：关节矩阵起点；y：贴图与实例化标志位（1 normal, 2 mr, 4 ao, 8 emissive, 16 instanced）；z：风格（0 PBR, 1 Toon, 2 Water, 3 Foliage）；w：打包的 normal_scale / occlusion_strength
    skin: vec4u,
    // x：变形差从第几个 vec4 开始，y：目标数，z：顶点数，w：实例化起点
    morph: vec4u,
    weights: array<vec4f, 2>,
    custom: vec4f,
}
struct Instance {
    model: mat4x4f,
    normal: mat4x4f,
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
@group(1) @binding(3) var<storage, read> instances: array<Instance>;
@group(2) @binding(0) var base_tex: texture_2d<f32>;
@group(2) @binding(1) var normal_tex: texture_2d<f32>;
@group(2) @binding(2) var mr_tex: texture_2d<f32>;
@group(2) @binding(3) var ao_tex: texture_2d<f32>;
@group(2) @binding(4) var emissive_tex: texture_2d<f32>;
@group(2) @binding(5) var base_sampler: sampler;

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

fn vertex_with(model: mat4x4f, normal_mat: mat4x4f, position: vec4f, normal: vec4f, uv: vec2f) -> VsOut {
    var world = model * position;
    // 植被风格（style == 3）：按局部高度施加柔和风吹摆动，根部固定、树冠/草尖随风轻晃（兼容米级与大比例世界）
    if d.skin.z == 3u {
        let t = g.ambient.w;
        let h = max(position.y, 0.0);
        let sway_weight = min(h * 0.12, 1.0) * (1.0 + min(h * 0.04, 2.5));
        let phase = world.x * 0.045 + world.z * 0.035 + t * 2.1;
        let gust = sin(phase) * 0.28 + sin(phase * 2.3 + 1.1) * 0.12;
        world.x += gust * sway_weight;
        world.z += cos(phase * 0.9) * 0.20 * sway_weight;
    }
    var o: VsOut;
    o.clip = g.view_proj * world;
    o.world = world.xyz;
    o.normal = (normal_mat * normal).xyz;
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
fn vs(v: VsIn, @builtin(instance_index) inst: u32) -> VsOut {
    var model = d.model;
    var normal_mat = d.normal;
    if (d.skin.y & 16u) != 0u {
        let item = instances[d.morph.w + inst];
        model = item.model;
        normal_mat = item.normal;
    }
    let p = morph(v.index, v.position, v.normal);
    return vertex_with(model, normal_mat, vec4f(p.position, 1.0), vec4f(p.normal, 0.0), v.uv);
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
    return vertex_with(d.model, d.normal, m * vec4f(p.position, 1.0), m * vec4f(p.normal, 0.0), v.uv);
}

const PI: f32 = 3.14159265;

/// 屏幕空间余切标架法线扰动（不需要顶点切线，兼容所有 glTF 模型与 GLES 3.1）。
fn perturb_normal(n: vec3f, dp1: vec3f, dp2: vec3f, duv1: vec2f, duv2: vec2f, map_n: vec3f, scale: f32) -> vec3f {
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let max_len2 = max(dot(t, t), dot(b, b));
    if max_len2 <= 1e-10 {
        return n;
    }
    let invmax = inverseSqrt(max_len2);
    let ts = normalize(vec3f((map_n.xy * 2.0 - 1.0) * scale, map_n.z * 2.0 - 1.0));
    return normalize(t * (invmax * ts.x) + b * (invmax * ts.y) + n * ts.z);
}

/// 水面程序化双层波纹法线扰动。
fn water_normal(n: vec3f, world: vec3f, time: f32) -> vec3f {
    let p1 = world.xz * 0.55 + vec2f(time * 0.7, time * 0.45);
    let p2 = world.xz * 1.15 - vec2f(time * 0.5, time * 0.85);
    let dx = cos(p1.x + sin(p1.y) * 0.6) * 0.07 + cos(p2.x - p2.y) * 0.045;
    let dz = sin(p1.y + cos(p1.x) * 0.6) * 0.07 + sin(p2.y + p2.x) * 0.045;
    return normalize(n + vec3f(dx, 0.0, dz));
}

fn shade(n: vec3f, v: vec3f, l: vec3f, radiance: vec3f, albedo: vec3f, metallic: f32, roughness: f32, style: u32) -> vec3f {
    // 卡通风格（Toon）：三阶柔和色带 + 硬朗高光 + 边缘轮廓光
    if style == 1u {
        let raw_nl = dot(n, l);
        let band1 = smoothstep(0.08, 0.16, raw_nl);
        let band2 = smoothstep(0.48, 0.56, raw_nl);
        let lit = 0.18 + band1 * 0.42 + band2 * 0.40;
        let h = normalize(v + l);
        let spec_band = smoothstep(0.92, 0.96, max(dot(n, h), 0.0)) * (1.0 - roughness * 0.7) * 0.35;
        let rim = pow(clamp(1.0 - max(dot(n, v), 0.0), 0.0, 1.0), 3.0) * max(raw_nl, 0.0) * 0.28;
        return (albedo * lit / PI + vec3f(spec_band + rim)) * radiance;
    }
    // 植被风格（Foliage）：包裹漫反射 + 逆光次表面透光
    if style == 3u {
        let wrap_nl = clamp((dot(n, l) + 0.38) / 1.38, 0.0, 1.0);
        let diffuse = albedo / PI * wrap_nl;
        let back = pow(clamp(dot(v, -l), 0.0, 1.0), 3.0) * 0.35;
        let translucency = albedo * vec3f(1.08, 1.18, 0.65) * back / PI;
        return (diffuse + translucency) * radiance;
    }
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

// NGE_CUSTOM_SHADE3D_HOOK

@fragment
fn fs(i: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    // d.skin.y 来自 uniform，对整个 draw 一致（满足 WebGPU / GLES 3.1 均匀控制流），跳过未使用的贴图采样与导数计算
    let flags = d.skin.y;
    let base_sample = textureSample(base_tex, base_sampler, i.uv);
    var normal_sample = vec3f(0.5, 0.5, 1.0);
    var dp1 = vec3f(0.0);
    var dp2 = vec3f(0.0);
    var duv1 = vec2f(0.0);
    var duv2 = vec2f(0.0);
    if (flags & 1u) != 0u {
        normal_sample = textureSample(normal_tex, base_sampler, i.uv).xyz;
        dp1 = dpdx(i.world);
        dp2 = dpdy(i.world);
        duv1 = dpdx(i.uv);
        duv2 = dpdy(i.uv);
    }
    var mr_sample = vec4f(1.0);
    if (flags & 2u) != 0u {
        mr_sample = textureSample(mr_tex, base_sampler, i.uv);
    }
    var ao_sample = 1.0;
    if (flags & 4u) != 0u {
        ao_sample = textureSample(ao_tex, base_sampler, i.uv).r;
    }
    var emissive_sample = vec3f(1.0);
    if (flags & 8u) != 0u {
        emissive_sample = textureSample(emissive_tex, base_sampler, i.uv).rgb;
    }

    let base = base_sample * d.base_color;
    if d.params.z >= 0.0 && base.a < d.params.z {
        discard;
    }
    let style = d.skin.z;
    let normal_scale = f32(d.skin.w & 0xffffu) / 4096.0;
    let ao_strength = f32((d.skin.w >> 16u) & 0xffffu) / 4096.0;

    var n = normalize(i.normal);
    // params.w > 0：镜像变换，光栅化的正反面和几何上的相反
    if front == (d.params.w > 0.0) {
        n = -n;
    }
    let geo_n = n;
    if (flags & 1u) != 0u {
        n = perturb_normal(n, dp1, dp2, duv1, duv2, normal_sample, normal_scale);
    }
    if style == 2u {
        n = water_normal(n, i.world, g.ambient.w);
    }
    // eye.w = 0：正交相机，所有片元的视线方向一样
    let v = normalize(select(g.eye.xyz - i.world, g.eye.xyz, g.eye.w == 0.0));
    var metallic = d.params.x;
    var roughness = d.params.y;
    if (flags & 2u) != 0u {
        roughness *= mr_sample.g;
        metallic *= mr_sample.b;
    }
    metallic = clamp(metallic, 0.0, 1.0);
    roughness = clamp(roughness, 0.04, 1.0);

    var albedo = base.rgb;
    if style == 2u {
        roughness = min(roughness, 0.08);
        let fresnel = pow(clamp(1.0 - max(dot(n, v), 0.0), 0.0, 1.0), 4.0);
        let shallow = mix(albedo, vec3f(0.32, 0.78, 0.88), 0.35);
        let sky_tint = mix(vec3f(0.45, 0.72, 0.92), g.sky[1].rgb, select(0.0, 0.7, g.env.w > 0.0));
        albedo = mix(shallow, sky_tint, fresnel * 0.55);
    }

    var ao = 1.0;
    if (flags & 4u) != 0u {
        ao = mix(1.0, ao_sample, ao_strength);
    }
    var emissive = d.emissive.rgb;
    if (flags & 8u) != 0u {
        emissive *= emissive_sample;
    }

    var color = g.ambient.rgb * albedo * ao + emissive;
    if g.env.w > 0.0 {
        color += environment(n, v, albedo, metallic, roughness) * ao;
    }
    for (var k = 0u; k < g.counts.x; k++) {
        let light = g.dirs[k];
        var radiance = light.color.rgb;
        if g.counts.z == k + 1u {
            radiance *= lit_directional(i.world, geo_n);
        }
        color += shade(n, v, light.to_light.xyz, radiance, albedo, metallic, roughness, style);
    }
    for (var k = 0u; k < g.counts.y; k++) {
        let light = g.points[k];
        let to = light.position_range.xyz - i.world;
        let dist2 = max(dot(to, to), 1e-4);
        var radiance = light.color.rgb * attenuation(dist2, light.position_range.w);
        let layer = g.point_layers[k / 4u][k % 4u];
        if layer > 0u {
            radiance *= lit_point(layer - 1u, i.world, geo_n, light.position_range.xyz);
        }
        color += shade(n, v, to * inverseSqrt(dist2), radiance, albedo, metallic, roughness, style);
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
            radiance *= lit_local(layer - 1u, i.world, geo_n, dot(-to, light.direction.xyz), g.spot_offset[k]);
        }
        color += shade(n, v, l, radiance, albedo, metallic, roughness, style);
    }
    color = apply_fog(color, i.world, g.fog, g.sh);
    if g.casters.y == 1u && g.counts.z > 0u {
        color *= cascade_tint(i.world);
    }
    var out_alpha = select(1.0, base.a, d.emissive.w > 0.0);
    // NGE_CUSTOM_SHADE3D_CALL
    color = apply_tonemap(color);
    return vec4f(color, out_alpha);
}

/// 真实线性 HDR 色调映射（g.sky[0].w: 0 关闭 / 1 ACES / 2 AgX / 3 Reinhard；g.sky[1].w: 曝光；g.sky[2].w: 饱和度）。
fn apply_tonemap(raw: vec3f) -> vec3f {
    let mode = g.sky[0].w;
    if mode < 0.5 {
        return raw;
    }
    let exposure = select(g.sky[1].w, 1.0, g.sky[1].w <= 0.0);
    let sat = select(g.sky[2].w, 1.0, g.sky[2].w <= 0.0);
    var c = max(raw * exposure, vec3f(0.0));
    if mode < 1.5 {
        c = clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), vec3f(0.0), vec3f(1.0));
    } else if mode < 2.5 {
        let m = mat3x3f(
            vec3f(0.84247906, 0.04232824, 0.04237565),
            vec3f(0.07843360, 0.87846864, 0.07843360),
            vec3f(0.07922375, 0.07916613, 0.87914297),
        );
        var x = clamp((log2(max(m * c, vec3f(1e-6))) + 12.47393) / 16.5, vec3f(0.0), vec3f(1.0));
        let x2 = x * x;
        let x4 = x2 * x2;
        x = 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
        let inv_m = mat3x3f(
            vec3f(1.19687900, -0.05289685, -0.05297163),
            vec3f(-0.09802088, 1.15190313, -0.09804345),
            vec3f(-0.09902974, -0.09896118, 1.15107367),
        );
        c = clamp(inv_m * x, vec3f(0.0), vec3f(1.0));
    } else {
        c = c / (vec3f(1.0) + c);
    }
    let luma = dot(c, vec3f(0.2126, 0.7152, 0.0722));
    return clamp(mix(vec3f(luma), c, sat), vec3f(0.0), vec3f(1.0));
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
        return vec4f(apply_tonemap(textureSampleLevel(sky_cube, env_sampler, d, 0.0).rgb), 1.0);
    }
    return vec4f(apply_tonemap(sky_gradient(d)), 1.0);
}
