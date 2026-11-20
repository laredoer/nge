// 屏幕空间环境光遮蔽（SSAO）：读取深度缓冲，按 4×4 Bayer 旋转 8 点盘估计局部凹陷与接触遮蔽，以乘法混合叠到画面上。

struct SsaoParams {
    // x: intensity, y: radius (uv), z: bias, w: unused
    params: vec4f,
    // xy: 1.0 / resolution, zw: resolution
    texel: vec4f,
}

@group(0) @binding(0) var<uniform> u: SsaoParams;
@group(0) @binding(1) var depth_tex: texture_depth_2d;

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: Out;
    o.clip = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

fn load_depth(uv: vec2f) -> f32 {
    let size = vec2i(u.texel.zw);
    let coord = clamp(vec2i(uv * u.texel.zw), vec2i(0), size - vec2i(1));
    return textureLoad(depth_tex, coord, 0);
}

@fragment
fn fs(i: Out) -> @location(0) vec4f {
    let d0 = load_depth(i.uv);
    // 天空 / 未绘制区域不遮蔽
    if d0 >= 0.9999 {
        return vec4f(1.0);
    }
    let p = vec2u(floor(i.uv * u.texel.zw)) % 4u;
    var bayer = array<f32, 16>(
        0.0, 8.0, 2.0, 10.0,
        12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0,
        15.0, 7.0, 13.0, 5.0,
    );
    let angle = (bayer[p.y * 4u + p.x] + 0.5) / 16.0 * 6.2831853;
    let c = cos(angle);
    let s = sin(angle);
    let rot = mat2x2f(c, s, -s, c);

    var offsets = array<vec2f, 4>(
        vec2f(0.35, 0.16),
        vec2f(-0.48, 0.34),
        vec2f(0.24, -0.68),
        vec2f(-0.74, -0.44),
    );

    // 随深度自适应缩小屏幕采样半径，保持近远一致的接触暗角
    let radius = u.params.y * clamp(1.15 - d0 * 0.95, 0.15, 1.0);
    let bias = u.params.z;
    let max_diff = max(bias * 30.0, 0.015);
    var occ = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let tap_uv = i.uv + (rot * offsets[k]) * radius;
        let d = load_depth(tap_uv);
        let diff = d0 - d;
        if diff > bias && diff < max_diff {
            let range_w = 1.0 - smoothstep(bias, max_diff, diff);
            occ += range_w;
        }
    }
    let ao = clamp(1.0 - (occ * 0.25) * u.params.x, 0.24, 1.0);
    return vec4f(ao, ao, ao, 1.0);
}
