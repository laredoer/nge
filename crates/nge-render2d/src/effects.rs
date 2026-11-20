//! 2D 特效的数据：着色器库、精灵的着色器和遮罩、2D 光照、后处理（见 docs/effects2d.md）。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::{Vec2, Vec3, Vec4};

use crate::occluders::MAX_OCCLUDER_EDGES;

/// 最多这么多盏灯，多的不画。
pub const MAX_LIGHTS: usize = 32;

/// 着色器库：名字 → WGSL 片段。精灵着色器定义 `fn effect(e: Effect) -> vec4f`，后处理定义 `fn post(p: Post) -> vec4f`。
/// 内置几个（见 [`Shaders::BUILTIN`]），游戏加的同名时用游戏的。
#[derive(Resource, Debug, Clone)]
pub struct Shaders {
    user: HashMap<String, String>,
    /// 提示过游戏写了叫 bloom 的着色器。
    warned_bloom: bool,
    /// 每次改动加一，渲染器按它丢掉编译过的管线。
    generation: u64,
}

impl Default for Shaders {
    fn default() -> Self {
        Self { user: HashMap::new(), warned_bloom: false, generation: 0 }
    }
}

impl Shaders {
    /// 内置的着色器：（名字，是不是后处理，默认参数，代码）。
    pub const BUILTIN: &[(&str, bool, [f32; 4], &str)] = &[
        // 往 params.rgb 的颜色混，混多少看 params.w
        ("flash", false, [1.0, 1.0, 1.0, 1.0], "fn effect(e: Effect) -> vec4f {\n    return vec4f(mix(e.color.rgb, e.params.rgb, e.params.w), e.color.a);\n}"),
        // 透明的地方挨着不透明的就描成 params.rgb，params.w 是几个贴图像素宽
        (
            "outline",
            false,
            [1.0, 1.0, 1.0, 1.0],
            "fn effect(e: Effect) -> vec4f {
    if e.color.a >= 0.5 {
        return e.color;
    }
    let texel = e.params.w / vec2f(textureDimensions(tex));
    let near = max(max(sample(e.uv + vec2f(texel.x, 0.0)).a, sample(e.uv - vec2f(texel.x, 0.0)).a),
                   max(sample(e.uv + vec2f(0.0, texel.y)).a, sample(e.uv - vec2f(0.0, texel.y)).a));
    if near >= 0.5 {
        return vec4f(e.params.rgb, e.tint.a);
    }
    return e.color;
}",
        ),
        // 贴图坐标按 sin 横向摆：幅度 params.x（贴图宽的比例）、竖直方向的频率 params.y、速度 params.z
        (
            "wave",
            false,
            [0.02, 10.0, 3.0, 0.0],
            "fn effect(e: Effect) -> vec4f {
    let dx = e.params.x * sin(e.local.y * e.params.y + e.time * e.params.z);
    return sample(e.uv + vec2f(dx * e.uv_size.x, 0.0)) * e.tint;
}",
        ),
        // 只留内切圆（当圆形遮罩用）
        ("circle", false, [0.0; 4], "fn effect(e: Effect) -> vec4f {\n    if length(e.local - vec2f(0.5)) > 0.5 {\n        return vec4f(0.0);\n    }\n    return e.color;\n}"),
        // 去色，params.x 是程度
        (
            "grayscale",
            true,
            [1.0, 0.0, 0.0, 0.0],
            "fn post(p: Post) -> vec4f {\n    let g = dot(p.color.rgb, vec3f(0.2126, 0.7152, 0.0722));\n    return vec4f(mix(p.color.rgb, vec3f(g), p.params.x), p.color.a);\n}",
        ),
        // 四角压暗：params.x 是程度，params.y 是从中心多远开始暗（0–1，到角是 1）
        (
            "vignette",
            true,
            [0.5, 0.6, 0.0, 0.0],
            "fn post(p: Post) -> vec4f {
    let d = length((p.uv - vec2f(0.5)) * 2.0) / sqrt(2.0);
    let k = 1.0 - p.params.x * smoothstep(p.params.y, 1.0, d);
    return vec4f(p.color.rgb * k, p.color.a);
}",
        ),
        // 像素化：params.x 是一格多少个屏幕像素
        (
            "pixelate",
            true,
            [6.0, 0.0, 0.0, 0.0],
            "fn post(p: Post) -> vec4f {\n    let n = max(floor(p.params.x), 1.0);\n    let px = floor(p.uv * p.size / n) * n + floor(n / 2.0) + 0.5;\n    return sample(px / p.size);\n}",
        ),
        // 色调映射：params.x 曝光（默认 1），params.y 曲线（0 ACES / 1 AgX / 2 Reinhard），params.z 饱和度（默认 1）
        (
            "tonemap",
            true,
            [1.0, 0.0, 1.0, 0.0],
            "fn post(p: Post) -> vec4f {
    let exposure = select(p.params.x, 1.0, p.params.x <= 0.0);
    let sat = select(p.params.z, 1.0, p.params.z <= 0.0);
    var c = max(p.color.rgb * exposure, vec3f(0.0));
    if p.params.y < 0.5 {
        c = clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), vec3f(0.0), vec3f(1.0));
    } else if p.params.y < 1.5 {
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
    c = clamp(mix(vec3f(luma), c, sat), vec3f(0.0), vec3f(1.0));
    return vec4f(c, p.color.a);
}",
        ),
        // 快速近似抗锯齿（FXAA）：params.x 子像素平滑（默认 0.75），params.y 边缘阈值（0.166），params.z 最小阈值（0.0833）
        (
            "fxaa",
            true,
            [0.75, 0.166, 0.0833, 0.0],
            "fn post(p: Post) -> vec4f {
    let px = 1.0 / max(p.size, vec2f(1.0));
    let luma_w = vec3f(0.299, 0.587, 0.114);
    let rgb_m = p.color.rgb;
    let rgb_nw = sample(p.uv + vec2f(-px.x, -px.y)).rgb;
    let rgb_ne = sample(p.uv + vec2f(px.x, -px.y)).rgb;
    let rgb_sw = sample(p.uv + vec2f(-px.x, px.y)).rgb;
    let rgb_se = sample(p.uv + vec2f(px.x, px.y)).rgb;
    let l_m = dot(rgb_m, luma_w);
    let l_nw = dot(rgb_nw, luma_w);
    let l_ne = dot(rgb_ne, luma_w);
    let l_sw = dot(rgb_sw, luma_w);
    let l_se = dot(rgb_se, luma_w);
    let l_min = min(l_m, min(min(l_nw, l_ne), min(l_sw, l_se)));
    let l_max = max(l_m, max(max(l_nw, l_ne), max(l_sw, l_se)));
    let range = l_max - l_min;
    let thresh = select(p.params.y, 0.166, p.params.y <= 0.0);
    let min_thresh = select(p.params.z, 0.0833, p.params.z <= 0.0);
    if range < max(min_thresh, l_max * thresh) {
        return p.color;
    }
    var dir = vec2f(-((l_nw + l_ne) - (l_sw + l_se)), (l_nw + l_sw) - (l_ne + l_se));
    let dir_reduce = max((l_nw + l_ne + l_sw + l_se) * 0.03125, 1.0 / 128.0);
    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + dir_reduce);
    dir = clamp(dir * rcp, vec2f(-8.0), vec2f(8.0)) * px;
    let rgb_a = 0.5 * (sample(p.uv + dir * (1.0 / 3.0 - 0.5)).rgb + sample(p.uv + dir * (2.0 / 3.0 - 0.5)).rgb);
    let rgb_b = rgb_a * 0.5 + 0.25 * (sample(p.uv + dir * -0.5).rgb + sample(p.uv + dir * 0.5).rgb);
    let l_b = dot(rgb_b, luma_w);
    let out_rgb = select(rgb_b, rgb_a, l_b < l_min || l_b > l_max);
    let blend = clamp(select(p.params.x, 0.75, p.params.x <= 0.0), 0.0, 1.0);
    return vec4f(mix(rgb_m, out_rgb, blend), p.color.a);
}",
        ),
    ];

    /// 内置的多遍后处理 bloom 的名字（不是一个着色器，游戏不能用这个名字）。
    pub const BLOOM: &str = "bloom";
    /// bloom 的默认参数：阈值、强度、级数。
    pub const BLOOM_PARAMS: [f32; 4] = [0.8, 1.0, 5.0, 0.0];

    /// 游戏加的着色器；名字已有的覆盖（包括内置的）。叫 `bloom` 的忽略（那是内置的多遍效果）。返回改没改。
    pub fn set(&mut self, mut user: HashMap<String, String>) -> bool {
        if user.remove(Self::BLOOM).is_some() && !std::mem::replace(&mut self.warned_bloom, true) {
            log::warn!("着色器库里的 {} 是内置的多遍后处理，游戏写的这个忽略", Self::BLOOM);
        }
        if user == self.user {
            return false;
        }
        self.user = user;
        self.generation += 1;
        true
    }

    /// 校验一段精灵着色器（定义 `effect`），不过时返回带行号的错误。游戏可以在测试里用它检查自己写的。
    pub fn check_sprite(code: &str) -> Result<(), String> {
        validate(&sprite_source(Some(code)), code)
    }

    /// 校验一段后处理着色器（定义 `post`）。
    pub fn check_post(code: &str) -> Result<(), String> {
        validate(&post_source(code), code)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 名字对应的代码，游戏的优先；不认识时是 `None`。
    pub fn code(&self, name: &str) -> Option<&str> {
        self.user.get(name).map(String::as_str).or_else(|| Self::builtin(name).map(|b| b.3))
    }

    /// 参数不写时用的默认值：内置的有自己的，游戏加的是 0。
    pub fn default_params(name: &str) -> Vec4 {
        if name == Self::BLOOM {
            return Vec4::from(Self::BLOOM_PARAMS);
        }
        Self::builtin(name).map_or(Vec4::ZERO, |b| Vec4::from(b.2))
    }

    /// 库里所有的（名字，代码），游戏的覆盖内置的，按名字排好。
    pub(crate) fn library(&self) -> Vec<(String, String)> {
        let builtin = Self::BUILTIN.iter().filter(|b| !self.user.contains_key(b.0)).map(|b| (b.0.to_owned(), b.3.to_owned()));
        let mut all: Vec<_> = builtin.chain(self.user.iter().map(|(n, c)| (n.clone(), c.clone()))).collect();
        all.sort();
        all
    }

    fn builtin(name: &str) -> Option<&'static (&'static str, bool, [f32; 4], &'static str)> {
        Self::BUILTIN.iter().find(|b| b.0 == name)
    }
}

/// 精灵用库里的哪个着色器画，`params` 交给它（`Effect.params`）。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct SpriteShader {
    pub name: String,
    pub params: Vec4,
}

/// 遮罩：`Write` 的精灵不画出来，只把它不透明（alpha ≥ 0.5）的地方记成遮罩；
/// `Inside` 的只画在遮罩里，`Outside` 的只画在遮罩外。同一层（z < 0、z ≥ 0 各一层）的遮罩合在一起。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Masking {
    Write,
    Inside,
    Outside,
}

/// 开着 2D 光照时这个精灵不受光（自己发光的东西、界面）。
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Unlit;

/// 2D 光照。`enabled` 时世界里的精灵（不是 [`Unlit`]、不是粒子 / 文字 / 调试线）的颜色乘上 `ambient` 加上照到它的灯。
/// 默认关着，环境光是白的（不变暗）。
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Lighting2d {
    pub enabled: bool,
    /// 线性 RGB，1 是不变暗。
    pub ambient: Vec3,
    pub lights: Vec<Light2d>,
}

impl Default for Lighting2d {
    fn default() -> Self {
        Self { enabled: false, ambient: Vec3::ONE, lights: Vec::new() }
    }
}

/// 点光源：中心最亮，到 `radius` 处减到 0（平方衰减）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light2d {
    pub position: Vec2,
    pub radius: f32,
    /// 线性 RGB。
    pub color: Vec3,
    pub intensity: f32,
    /// 离画面多高（point），只影响带法线图的精灵：越低越像从侧面打过来。
    pub height: f32,
    /// 被 [`crate::Occluder2d`] 挡住。
    pub shadows: bool,
}

impl Default for Light2d {
    fn default() -> Self {
        Self { position: Vec2::ZERO, radius: 0.0, color: Vec3::ONE, intensity: 1.0, height: 0.0, shadows: false }
    }
}

/// 后处理：整帧画完以后按顺序过一遍这些着色器。空的时候不做后处理（直接画到屏幕上）。
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct PostEffects(pub Vec<PostEffect>);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PostEffect {
    /// 着色器库里的后处理，或者内置的多遍效果 `bloom`。
    pub shader: String,
    pub params: Vec4,
    pub layer: PostLayer,
}

/// 后处理用在哪：整屏，还是只给 z < 0 的背景层。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PostLayer {
    #[default]
    Screen,
    Back,
}

impl PostEffects {
    /// 用在这一层的后处理。
    pub fn layer(&self, layer: PostLayer) -> impl Iterator<Item = &PostEffect> {
        self.0.iter().filter(move |e| e.layer == layer)
    }
}

/// 光照打进 uniform 的样子：每盏灯三个 vec4（位置、半径、强度；颜色、高度；shadows），挡光的边每条一个 vec4。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Globals {
    pub view_proj: [f32; 16],
    /// 和 3D 混排时，3D 相机的矩阵：精灵的深度按它算。
    pub depth_view_proj: [f32; 16],
    pub time: f32,
    pub light_count: u32,
    pub lighting: u32,
    /// 1：和 3D 混排，按 `depth_view_proj` 写深度。
    pub depth: u32,
    pub ambient: [f32; 4],
    pub edge_count: u32,
    pub _pad: [u32; 3],
    pub lights: [[f32; 12]; MAX_LIGHTS],
    pub edges: [[f32; 4]; MAX_OCCLUDER_EDGES],
}

impl Globals {
    /// 半径不是正数、不是有限数的灯跳过；超过 [`MAX_LIGHTS`] 盏的只取前面的。`depth` 是混排时 3D 相机的矩阵。
    /// `edges` 是 [`crate::occluders::pack_edges`] 打好的挡光的边（多了的不要）。
    pub fn new(view_proj: [f32; 16], depth: Option<[f32; 16]>, time: f32, lighting: &Lighting2d, edges: &[[f32; 4]]) -> Self {
        let mut g = Self { view_proj, time, ..bytemuck::Zeroable::zeroed() };
        if let Some(m) = depth {
            (g.depth_view_proj, g.depth) = (m, 1);
        }
        if !lighting.enabled {
            return g;
        }
        g.lighting = 1;
        g.ambient = lighting.ambient.extend(1.0).into();
        let usable = lighting.lights.iter().filter(|l| l.radius.is_finite() && l.radius > 0.0 && l.position.is_finite());
        for (slot, l) in g.lights.iter_mut().zip(usable) {
            let shadows = if l.shadows { 1.0 } else { 0.0 };
            *slot = [l.position.x, l.position.y, l.radius, l.intensity, l.color.x, l.color.y, l.color.z, l.height, shadows, 0.0, 0.0, 0.0];
            g.light_count += 1;
        }
        for (slot, edge) in g.edges.iter_mut().zip(edges) {
            *slot = *edge;
            g.edge_count += 1;
        }
        g
    }
}

/// 精灵着色器的完整 WGSL：公用的顶点、片元入口加上游戏的 `effect`（`None` 是原样画）。
pub(crate) fn sprite_source(effect: Option<&str>) -> String {
    let effect = effect.unwrap_or("fn effect(e: Effect) -> vec4f {\n    return e.color;\n}");
    format!("{SPRITE_HEAD}\n{effect}\n{SPRITE_TAIL}")
}

/// 后处理着色器的完整 WGSL。
pub(crate) fn post_source(post: &str) -> String {
    format!("{POST_HEAD}\n{post}\n{POST_TAIL}")
}

/// 模板自己声明的绑定数（精灵 4 个、后处理 3 个）。
const SPRITE_BINDINGS: usize = 4;
const POST_BINDINGS: usize = 3;

/// 用 naga 解析、校验一遍；不过时返回能给人看的错误（附上游戏写的那段从完整着色器的第几行开始）。
/// 管线布局是固定的：游戏自己声明绑定、`override`、`var<immediate>` 时 naga 能过、建管线会失败，这里一并拒掉。
pub(crate) fn validate(source: &str, user: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| shift(&e.emit_to_string(source), source, user))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| shift(&e.emit_to_string(source), source, user))?;
    let bindings = module.global_variables.iter().filter(|(_, g)| g.binding.is_some()).count();
    let immediate = module.global_variables.iter().any(|(_, g)| g.space == naga::AddressSpace::Immediate);
    let expected = if source.starts_with(SPRITE_HEAD) { SPRITE_BINDINGS } else { POST_BINDINGS };
    if bindings != expected || immediate || !module.overrides.is_empty() {
        return Err(shift("不能自己声明绑定（@group / @binding）、override 或 var<immediate>：参数用 params 传", source, user));
    }
    Ok(())
}

/// 错误信息里的行号减去模板前面的行数，对上游戏写的代码。
fn shift(message: &str, source: &str, user: &str) -> String {
    let before = source.find(user).map_or(0, |at| source[..at].lines().count());
    format!("{message}\n（游戏写的代码从完整着色器的第 {} 行开始）", before + 1)
}

pub(crate) const SPRITE_HEAD: &str = r#"
struct Light { a: vec4f, b: vec4f, c: vec4f }
struct Globals {
    view_proj: mat4x4f,
    depth_view_proj: mat4x4f,
    time: f32,
    light_count: u32,
    lighting: u32,
    depth: u32,
    ambient: vec4f,
    edge_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    lights: array<Light, 32>,
    edges: array<vec4f, 128>,
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var nge_normal_tex: texture_2d<f32>;

struct Inst {
    @location(0) pos: vec2f,
    @location(1) size: vec2f,
    @location(2) rotation_z: vec2f,
    @location(3) uv: vec4f,
    @location(4) color: vec4f,
    @location(5) flags: u32,
    @location(6) params: vec4f,
}
struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
    @location(2) world: vec2f,
    @location(3) local: vec2f,
    @location(4) params: vec4f,
    @location(5) @interpolate(flat) flags: u32,
    @location(6) @interpolate(flat) uv_size: vec2f,
    /// 精灵的 cos、sin（旋转）和宽、高的正负（镜像），法线图按它转。
    @location(7) @interpolate(flat) basis: vec4f,
}

/// 给 `effect` 的东西：`color` 是贴图颜色乘上精灵颜色，`tint` 是精灵颜色，`uv` 是贴图坐标（图集里的），
/// `uv_size` 是这一格在贴图坐标里多大，`local` 是在精灵里的位置（左下 0、右上 1），`world` 是世界坐标，`time` 是秒。
struct Effect {
    color: vec4f,
    tint: vec4f,
    uv: vec2f,
    uv_size: vec2f,
    local: vec2f,
    world: vec2f,
    params: vec4f,
    time: f32,
}

/// 在任意贴图坐标取色（不受控制流限制）。
fn sample(uv: vec2f) -> vec4f {
    return textureSampleLevel(tex, samp, uv, 0.0);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> Out {
    var corners = array<vec2f, 6>(
        vec2f(-0.5, -0.5), vec2f(0.5, -0.5), vec2f(0.5, 0.5),
        vec2f(-0.5, -0.5), vec2f(0.5, 0.5), vec2f(-0.5, 0.5),
    );
    let c = corners[vi];
    let local = c * i.size;
    let s = sin(i.rotation_z.x);
    let k = cos(i.rotation_z.x);
    let p = vec2f(local.x * k - local.y * s, local.x * s + local.y * k) + i.pos;
    var out: Out;
    out.clip = globals.view_proj * vec4f(p, 0.0, 1.0);
    // 和 3D 混排：深度按精灵的 z 在 3D 相机里算，超出前后范围的贴在最前 / 最后（不裁掉）
    if globals.depth != 0u {
        let d = globals.depth_view_proj * vec4f(p, i.rotation_z.y, 1.0);
        out.clip.z = clamp(d.z / d.w, 0.0, 1.0) * out.clip.w;
    }
    // 世界 y 朝上、图片 v 朝下
    out.uv = i.uv.xy + vec2f(c.x + 0.5, 0.5 - c.y) * i.uv.zw;
    out.color = i.color;
    out.world = p;
    out.local = c + vec2f(0.5);
    out.params = i.params;
    out.flags = i.flags;
    out.uv_size = i.uv.zw;
    out.basis = vec4f(k, s, select(1.0, -1.0, i.size.x < 0.0), select(1.0, -1.0, i.size.y < 0.0));
    return out;
}
"#;

const SPRITE_TAIL: &str = r#"
fn shade(in: Out) -> vec4f {
    var e: Effect;
    e.color = textureSample(tex, samp, in.uv) * in.color;
    e.tint = in.color;
    e.uv = in.uv;
    e.uv_size = in.uv_size;
    e.local = in.local;
    e.world = in.world;
    e.params = in.params;
    e.time = globals.time;
    return effect(e);
}

fn nge_cross2(a: vec2f, b: vec2f) -> f32 {
    return a.x * b.y - a.y * b.x;
}

/// 从 `p` 到灯心 `l` 的线段穿过某条挡光的边。
fn nge_blocked(p: vec2f, l: vec2f) -> bool {
    let r = l - p;
    for (var i = 0u; i < globals.edge_count; i++) {
        let e = globals.edges[i];
        let s = e.zw - e.xy;
        let d = nge_cross2(r, s);
        if abs(d) < 1e-6 {
            continue;
        }
        let t = nge_cross2(e.xy - p, s) / d;
        let u = nge_cross2(e.xy - p, r) / d;
        // 边的两端算在内（光不会从方块的角缝里漏过去），线段两头（这一点和灯心）不算
        if t > 0.0 && t < 1.0 && u >= 0.0 && u <= 1.0 {
            return true;
        }
    }
    return false;
}

/// 照到这一点的光：环境光加上每盏灯（平方衰减）。`bumpy` 时再乘法线和指向灯的方向的夹角；`shadowed` 时开了 shadows 的灯会被挡。
fn light(world: vec2f, normal: vec3f, bumpy: bool, shadowed: bool) -> vec3f {
    var sum = globals.ambient.rgb;
    for (var i = 0u; i < globals.light_count; i++) {
        let l = globals.lights[i];
        let f = clamp(1.0 - distance(world, l.a.xy) / l.a.z, 0.0, 1.0);
        var k = f * f;
        if k <= 0.0 {
            continue;
        }
        if bumpy {
            // 高度至少一点点：像素正好在灯心、高度 0 时不出 NaN
            k *= max(dot(normal, normalize(vec3f(l.a.xy - world, max(l.b.w, 1e-3)))), 0.0);
            if k <= 0.0 {
                continue;
            }
        }
        if shadowed && l.c.x != 0.0 && nge_blocked(world, l.a.xy) {
            continue;
        }
        sum += l.b.rgb * l.a.w * k;
    }
    return sum;
}

/// 法线图里的法线（图片坐标：绿色朝上、蓝色朝外）按精灵的镜像和旋转变到世界里。
fn nge_world_normal(in: Out, texel: vec3f) -> vec3f {
    var n = texel * 2.0 - 1.0;
    n = vec3f(n.x * in.basis.z, n.y * in.basis.w, n.z);
    let k = in.basis.x;
    let s = in.basis.y;
    return normalize(vec3f(n.x * k - n.y * s, n.x * s + n.y * k, n.z));
}

@fragment
fn fs(in: Out) -> @location(0) vec4f {
    var c = shade(in);
    // 在一致的控制流里取样；没有法线图时绑的是白色，不用
    let texel = textureSample(nge_normal_tex, samp, in.uv).rgb;
    if globals.lighting != 0u && (in.flags & 1u) != 0u {
        let bumpy = (in.flags & 2u) != 0u;
        c = vec4f(c.rgb * light(in.world, nge_world_normal(in, texel), bumpy, (in.flags & 4u) == 0u), c.a);
    }
    return c;
}

/// 遮罩：只写模板，透明的地方不算。
@fragment
fn fs_mask(in: Out) -> @location(0) vec4f {
    let c = shade(in);
    if c.a < 0.5 {
        discard;
    }
    return c;
}
"#;

pub(crate) const POST_HEAD: &str = r#"
struct PostGlobals { params: vec4f, size: vec2f, time: f32, _pad: f32 }
@group(0) @binding(0) var<uniform> globals: PostGlobals;
@group(0) @binding(1) var frame: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

/// 给 `post` 的东西：`color` 是这个像素画出来的颜色，`uv` 是屏幕上的位置（左上 0、右下 1），`size` 是屏幕像素数。
struct Post {
    color: vec4f,
    uv: vec2f,
    size: vec2f,
    params: vec4f,
    time: f32,
}

/// 在屏幕的任意位置取色。
fn sample(uv: vec2f) -> vec4f {
    return textureSampleLevel(frame, samp, uv, 0.0);
}

struct Out {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
}

/// 盖住全屏的一个三角形。
@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: Out;
    out.clip = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2f(p.x, 1.0 - p.y);
    return out;
}
"#;

const POST_TAIL: &str = r#"
@fragment
fn fs(in: Out) -> @location(0) vec4f {
    var p: Post;
    p.color = sample(in.uv);
    p.uv = in.uv;
    p.size = globals.size;
    p.params = globals.params;
    p.time = globals.time;
    return post(p);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_shaders_compile() {
        assert!(validate(&sprite_source(None), "").is_ok());
        for &(name, post, _, code) in Shaders::BUILTIN {
            let source = if post { post_source(code) } else { sprite_source(Some(code)) };
            if let Err(e) = validate(&source, code) {
                panic!("内置着色器 {name} 编不过：{e}");
            }
        }
    }

    #[test]
    fn bad_shaders_are_reported_with_the_user_line() {
        let code = "fn effect(e: Effect) -> vec4f {\n    return e.colour;\n}";
        let source = sprite_source(Some(code));
        let err = validate(&source, code).unwrap_err();
        assert!(err.contains("colour"), "{err}");
        let start = source[..source.find(code).unwrap()].lines().count() + 1;
        assert!(err.contains(&format!("第 {start} 行开始")), "{err}");
        // 没写 effect、返回类型不对
        assert!(Shaders::check_sprite("fn nothing() {}").is_err());
        assert!(Shaders::check_post("fn post(p: Post) -> f32 { return 1.0; }").is_err());
        assert!(Shaders::check_post(Shaders::BUILTIN[0].3).is_err(), "精灵着色器不能当后处理");
        assert!(Shaders::check_sprite(Shaders::code(&Shaders::default(), "vignette").unwrap()).is_err(), "后处理不能当精灵着色器");
        // naga 能过、建管线会失败的：自己声明绑定、override、immediate
        let extra = "@group(2) @binding(0) var<uniform> u: vec4f;\nfn effect(e: Effect) -> vec4f { return e.color * u; }";
        assert!(Shaders::check_sprite(extra).unwrap_err().contains("不能自己声明绑定"));
        assert!(Shaders::check_sprite("@group(0) @binding(0) var<uniform> again: vec4f;\nfn effect(e: Effect) -> vec4f { return e.color; }").is_err(), "同一个绑定换个名字");
        assert!(Shaders::check_sprite("override k: f32;\nfn effect(e: Effect) -> vec4f { return e.color * k; }").is_err());
        assert!(Shaders::check_post("@group(0) @binding(3) var<uniform> u: vec4f;\nfn post(p: Post) -> vec4f { return p.color * u; }").is_err());
        assert!(Shaders::check_sprite("var<private> k: f32 = 1.0;\nfn effect(e: Effect) -> vec4f { return e.color * k; }").is_ok(), "不带绑定的全局变量可以");
    }

    #[test]
    fn bloom_is_reserved_and_has_its_own_defaults() {
        let mut shaders = Shaders::default();
        assert!(!shaders.set(HashMap::from([("bloom".to_owned(), "fn post(p: Post) -> vec4f { return p.color; }".to_owned())])), "叫 bloom 的忽略");
        assert!(shaders.code("bloom").is_none());
        assert_eq!(Shaders::default_params("bloom"), Vec4::new(0.8, 1.0, 5.0, 0.0));
        let effects = PostEffects(vec![PostEffect { shader: "a".into(), ..Default::default() }, PostEffect { shader: "b".into(), layer: PostLayer::Back, ..Default::default() }]);
        assert_eq!(effects.layer(PostLayer::Back).map(|e| e.shader.as_str()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(effects.layer(PostLayer::Screen).map(|e| e.shader.as_str()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn user_shaders_override_and_bump_the_generation() {
        let mut shaders = Shaders::default();
        assert!(shaders.code("flash").unwrap().contains("mix"));
        assert!(shaders.code("nope").is_none());
        let user = HashMap::from([("flash".to_owned(), "mine".to_owned()), ("glow".to_owned(), "g".to_owned())]);
        assert!(shaders.set(user.clone()));
        assert_eq!((shaders.code("flash"), shaders.code("glow"), shaders.generation()), (Some("mine"), Some("g"), 1));
        assert!(!shaders.set(user), "一样的不算改");
        assert_eq!(shaders.generation(), 1);
        assert!(shaders.set(HashMap::new()));
        assert!(shaders.code("flash").unwrap().contains("mix"), "去掉以后回到内置的");
        assert_eq!(Shaders::default_params("vignette"), Vec4::new(0.5, 0.6, 0.0, 0.0));
        assert_eq!(Shaders::default_params("glow"), Vec4::ZERO);
    }

    #[test]
    fn globals_pack_lights() {
        assert_eq!(size_of::<Globals>(), 64 + 64 + 16 + 16 + 16 + 32 * 48 + 128 * 16, "和 WGSL 的布局一样");
        let edge = [[1.0, 2.0, 3.0, 4.0]];
        let off = Globals::new([0.0; 16], None, 1.5, &Lighting2d { enabled: false, ambient: Vec3::ONE, lights: vec![] }, &edge);
        assert_eq!((off.lighting, off.light_count, off.time, off.depth, off.edge_count), (0, 0, 1.5, 0, 0));
        let mixed = Globals::new([0.0; 16], Some([2.0; 16]), 0.0, &Lighting2d::default(), &[]);
        assert_eq!((mixed.depth, mixed.depth_view_proj), (1, [2.0; 16]));
        let light = |x: f32, radius: f32| Light2d { position: Vec2::new(x, 2.0), radius, color: Vec3::new(1.0, 0.5, 0.0), intensity: 3.0, height: 7.0, shadows: x == 1.0 };
        let mut lights: Vec<_> = (0..40).map(|i| light(i as f32, 10.0)).collect();
        lights.insert(0, light(0.0, 0.0));
        lights.insert(1, light(0.0, f32::NAN));
        let g = Globals::new([0.0; 16], None, 0.0, &Lighting2d { enabled: true, ambient: Vec3::splat(0.2), lights }, &edge);
        assert_eq!((g.lighting, g.light_count as usize), (1, MAX_LIGHTS));
        assert_eq!(g.ambient, [0.2, 0.2, 0.2, 1.0]);
        assert_eq!(g.lights[0], [0.0, 2.0, 10.0, 3.0, 1.0, 0.5, 0.0, 7.0, 0.0, 0.0, 0.0, 0.0], "半径不对的跳过");
        assert_eq!(g.lights[1][8], 1.0, "shadows");
        assert_eq!((g.edge_count, g.edges[0]), (1, edge[0]));
        assert_eq!(g.lights[31][0], 31.0);
    }
}
