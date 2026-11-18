// 调试画线：顶点已经是世界坐标，颜色不受光照。

struct View {
    view_proj: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;

struct In {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
}

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs(i: In) -> Out {
    var o: Out;
    o.clip = view.view_proj * vec4<f32>(i.position, 1.0);
    o.color = i.color;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return i.color;
}
