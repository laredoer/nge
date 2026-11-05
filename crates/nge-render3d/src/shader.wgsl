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
}
struct Draw {
    model: mat4x4f,
    normal: mat4x4f,
    base_color: vec4f,
    // metallic, roughness, alpha_cutoff（< 0 表示不透明）, 镜像（> 0）
    params: vec4f,
    emissive: vec4f,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<uniform> d: Draw;
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

@vertex
fn vs(v: VsIn) -> VsOut {
    let world = d.model * vec4f(v.position, 1.0);
    var o: VsOut;
    o.clip = g.view_proj * world;
    o.world = world.xyz;
    o.normal = (d.normal * vec4f(v.normal, 0.0)).xyz;
    o.uv = v.uv;
    return o;
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
        color += shade(n, v, light.to_light.xyz, light.color.rgb, base.rgb, metallic, roughness);
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
