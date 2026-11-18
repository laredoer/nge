// 朝着相机的方片和带子：顶点已经在 CPU 上展开好，颜色 = 贴图 × 顶点颜色，不受光照，受雾影响。前面拼着 fog.wgsl。

struct View {
    view_proj: mat4x4<f32>,
    fog: Fog,
    sh: array<vec4f, 9>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct In {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
}

@vertex
fn vs(i: In) -> Out {
    var o: Out;
    o.clip = view.view_proj * vec4<f32>(i.position, 1.0);
    o.uv = i.uv;
    o.color = i.color;
    o.world = i.position;
    return o;
}

/// 透明混合：颜色往雾色混。
@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    return vec4<f32>(apply_fog(c.rgb, i.world, view.fog, view.sh), c.a);
}

/// 叠加：加的是光，被雾遮住多少就少加多少。
@fragment
fn fs_add(i: Out) -> @location(0) vec4<f32> {
    let c = textureSample(tex, samp, i.uv) * i.color;
    return vec4<f32>(c.rgb * (1.0 - fog_amount(view.fog, i.world)), c.a);
}
