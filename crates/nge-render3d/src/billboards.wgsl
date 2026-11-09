// 朝着相机的方片和带子：顶点已经在 CPU 上展开好，颜色 = 贴图 × 顶点颜色，不受光照。

struct View {
    view_proj: mat4x4<f32>,
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
}

@vertex
fn vs(i: In) -> Out {
    var o: Out;
    o.clip = view.view_proj * vec4<f32>(i.position, 1.0);
    o.uv = i.uv;
    o.color = i.color;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv) * i.color;
}
