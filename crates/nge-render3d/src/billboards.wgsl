// 方片和带子：顶点已经在 CPU 上展开好，颜色 = 贴图（序列帧两帧混合）× 顶点颜色，可选受光（环境光 + 方向光）、软化，受雾影响。前面拼着 fog.wgsl。

struct DirLight { to_light: vec4f, color: vec4f }

struct View {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // 片元在相机前方的距离 = dot(xyz, 位置) + w
    forward: vec4f,
    ambient: vec4f,
    // x：环境漫反射强度，w > 0：有环境光照
    env: vec4f,
    // x：方向光数量
    counts: vec4u,
    dirs: array<DirLight, 4>,
    fog: Fog,
    sh: array<vec4f, 9>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

const INV_PI: f32 = 0.31830989;

struct In {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) uv2: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) normal: vec3<f32>,
    // 帧混合、软化距离、受光
    @location(5) params: vec4<f32>,
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
    @location(3) uv2: vec2<f32>,
    @location(4) normal: vec3<f32>,
    @location(5) params: vec4<f32>,
}

@vertex
fn vs(i: In) -> Out {
    var o: Out;
    o.clip = view.view_proj * vec4<f32>(i.position, 1.0);
    o.uv = i.uv;
    o.uv2 = i.uv2;
    o.color = i.color;
    o.world = i.position;
    o.normal = i.normal;
    o.params = i.params;
    return o;
}

// SOFT_BEGIN
@group(0) @binding(1) var scene_depth: texture_depth_2d;

/// 软化：片元离后面不透明表面（这个像素的深度还原成世界坐标）多远，不到 `soft` 时按比例淡出。
fn soft_fade(clip: vec4f, world: vec3f, soft: f32) -> f32 {
    if soft <= 0.0 {
        return 1.0;
    }
    let size = vec2f(textureDimensions(scene_depth));
    let d = textureLoad(scene_depth, vec2i(clip.xy), 0);
    let ndc = vec2f(clip.x / size.x * 2.0 - 1.0, 1.0 - clip.y / size.y * 2.0);
    let p = view.inv_view_proj * vec4f(ndc, d, 1.0);
    let behind = dot(view.forward.xyz, p.xyz / p.w - world);
    return clamp(behind / soft, 0.0, 1.0);
}
// SOFT_END

/// 受光：常量环境光 + 天空 SH 辐照度 + 方向光的包裹 lambert（方片很薄，背光面不是死黑）。
fn lighting(normal: vec3f) -> vec3f {
    let n = normalize(normal);
    var light = view.ambient.rgb;
    if view.env.w > 0.0 {
        light += max(sh_irradiance(view.sh, n), vec3f(0.0)) * INV_PI * view.env.x;
    }
    for (var k = 0u; k < view.counts.x; k++) {
        let w = dot(n, view.dirs[k].to_light.xyz) * 0.5 + 0.5;
        light += view.dirs[k].color.rgb * w * w * INV_PI;
    }
    return light;
}

fn shade(i: Out) -> vec4f {
    // 两帧都采样再混（textureSample 要在统一的控制流里）
    var c = mix(textureSample(tex, samp, i.uv), textureSample(tex, samp, i.uv2), i.params.x) * i.color;
    if i.params.z > 0.0 {
        c = vec4f(c.rgb * lighting(i.normal), c.a);
    }
    return vec4f(c.rgb, c.a * soft_fade(i.clip, i.world, i.params.y));
}

/// 透明混合：颜色往雾色混。
@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let c = shade(i);
    return vec4<f32>(apply_fog(c.rgb, i.world, view.fog, view.sh), c.a);
}

/// 叠加：加的是光，被雾遮住多少就少加多少。
@fragment
fn fs_add(i: Out) -> @location(0) vec4<f32> {
    let c = shade(i);
    return vec4<f32>(c.rgb * (1.0 - fog_amount(view.fog, i.world)), c.a);
}
