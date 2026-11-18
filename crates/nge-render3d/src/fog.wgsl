// 雾和 SH9 天空色（见 fog.rs、ibl.rs）：网格和粒子方片的着色器都拼上这一段。

struct Fog {
    // rgb 雾色，w 往天空色混多少
    color: vec4f,
    // 模式（0 没有、1 线性、2 指数、3 指数平方）、两个参数、w 开没开
    params: vec4f,
    // 高度雾的 base、falloff、density，w 最浓多少
    height: vec4f,
    // 相机位置，w = 1 按距离、0 按沿前方的距离
    camera: vec4f,
    forward: vec4f,
}

/// SH9 在方向 `d` 处的辐射度（模糊过的天空色）；系数顺序和 ibl.rs 的 `sh_basis` 一样。
fn sh_radiance(sh: array<vec4f, 9>, d: vec3f) -> vec3f {
    return sh_weighted(sh, d, vec3f(1.0));
}

/// SH9 在法线 `n` 处的辐照度：各阶乘上余弦卷积的系数 π、2π/3、π/4。
fn sh_irradiance(sh: array<vec4f, 9>, n: vec3f) -> vec3f {
    return sh_weighted(sh, n, vec3f(3.14159265, 2.0943951, 0.78539816));
}

fn sh_weighted(sh: array<vec4f, 9>, d: vec3f, band: vec3f) -> vec3f {
    var c = sh[0].rgb * 0.282095 * band.x;
    c += (sh[1].rgb * d.y + sh[2].rgb * d.z + sh[3].rgb * d.x) * 0.488603 * band.y;
    c += (sh[4].rgb * 1.092548 * d.x * d.y + sh[5].rgb * 1.092548 * d.y * d.z + sh[6].rgb * 0.315392 * (3.0 * d.z * d.z - 1.0)
        + sh[7].rgb * 1.092548 * d.x * d.z + sh[8].rgb * 0.546274 * (d.x * d.x - d.y * d.y)) * band.z;
    return c;
}

/// 片元 `world` 处雾有多浓（0–最浓），和 fog.rs 的 `Fog::amount` 是同一个公式。
fn fog_amount(f: Fog, world: vec3f) -> f32 {
    if f.params.w == 0.0 {
        return 0.0;
    }
    let along = dot(world, f.forward.xyz) + f.forward.w;
    let d = max(select(along, length(world - f.camera.xyz), f.camera.w > 0.0), 0.0);
    var by = 0.0;
    let mode = u32(f.params.x + 0.5);
    if mode == 1u {
        by = select(select(0.0, 1.0, d > f.params.y), clamp((d - f.params.y) / (f.params.z - f.params.y), 0.0, 1.0), f.params.z > f.params.y);
    } else if mode == 2u {
        by = 1.0 - exp(-max(f.params.y, 0.0) * d);
    } else if mode == 3u {
        let x = max(f.params.y, 0.0) * d;
        by = 1.0 - exp(-x * x);
    }
    var depth = 0.0;
    if f.height.z > 0.0 {
        let falloff = max(f.height.y, 0.0);
        let start = f.height.z * exp(-falloff * (f.camera.y - f.height.x));
        let x = falloff * (world.y - f.camera.y);
        // (1 - e^(-x)) / x，x 很小时取极限
        let average = select((1.0 - exp(-x)) / x, 1.0 - x * 0.5, abs(x) < 1e-4);
        depth = start * average * d;
    }
    return clamp((1.0 - (1.0 - by) * exp(-depth)) * f.height.w, 0.0, 1.0);
}

/// 把雾混进颜色：雾色按 `color.w` 往这个方向的天空色混。
fn apply_fog(color: vec3f, world: vec3f, f: Fog, sh: array<vec4f, 9>) -> vec3f {
    let amount = fog_amount(f, world);
    if amount <= 0.0 {
        return color;
    }
    var tint = f.color.rgb;
    if f.color.w > 0.0 {
        let dir = normalize(world - f.camera.xyz + vec3f(0.0, 0.0, 1e-6));
        tint = mix(tint, max(sh_radiance(sh, dir), vec3f(0.0)), f.color.w);
    }
    return mix(color, tint, amount);
}
