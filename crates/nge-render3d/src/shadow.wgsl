// 阴影 pass：只写深度。Draw、关节矩阵、变形差和 shader.wgsl 的一样。

struct Draw {
    model: mat4x4f,
    normal: mat4x4f,
    base_color: vec4f,
    params: vec4f,
    emissive: vec4f,
    skin: vec4u,
    morph: vec4u,
    weights: array<vec4f, 2>,
}

@group(0) @binding(0) var<uniform> light: mat4x4f;
@group(1) @binding(0) var<uniform> d: Draw;
@group(1) @binding(1) var<storage, read> joints: array<mat4x4f>;
@group(1) @binding(2) var<storage, read> morphs: array<vec4f>;

/// 按权重加上每个变形目标的位置差（法线用不到）。
fn morph(index: u32, position: vec3f) -> vec4f {
    var p = position;
    for (var t = 0u; t < d.morph.y; t++) {
        p += morphs[d.morph.x + (t * d.morph.z + index) * 2u].xyz * d.weights[t / 4u][t % 4u];
    }
    return vec4f(p, 1.0);
}

@vertex
fn vs(@location(0) position: vec3f, @builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    return light * d.model * morph(index, position);
}

@vertex
fn vs_skinned(@location(0) position: vec3f, @location(3) j: vec4u, @location(4) w: vec4f, @builtin(vertex_index) index: u32) -> @builtin(position) vec4f {
    let o = d.skin.x;
    var m = joints[o + j.x] * w.x + joints[o + j.y] * w.y + joints[o + j.z] * w.z + joints[o + j.w] * w.w;
    // 没有权重的顶点不动
    if dot(w, vec4f(1.0)) <= 0.0 {
        m = mat4x4f(vec4f(1.0, 0.0, 0.0, 0.0), vec4f(0.0, 1.0, 0.0, 0.0), vec4f(0.0, 0.0, 1.0, 0.0), vec4f(0.0, 0.0, 0.0, 1.0));
    }
    return light * d.model * m * morph(index, position);
}
