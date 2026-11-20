//! 自定义 3D 材质着色器：游戏注册 WGSL 片段 `fn shade3d(s: Surface) -> vec4f`，在完整 PBR/光照/阴影/雾计算之后、色调映射之前介入着色。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::Vec4;

/// 3D 着色器库：名字 → WGSL 片段（定义 `fn shade3d(s: Surface) -> vec4f`）。
#[derive(Resource, Debug, Clone, Default)]
pub struct Shaders3d {
    user: HashMap<String, String>,
    /// 每次改动加一，渲染器按它清掉旧的自定义管线缓存。
    generation: u64,
}

impl Shaders3d {
    /// 替换整张自定义 3D 着色器表；内容没变时返回 `false`（不重编）。
    pub fn set(&mut self, user: HashMap<String, String>) -> bool {
        if user == self.user {
            return false;
        }
        self.user = user;
        self.generation += 1;
        true
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn code(&self, name: &str) -> Option<&str> {
        self.user.get(name).map(String::as_str)
    }

    /// 校验一段自定义 3D 着色器片段（定义 `fn shade3d(s: Surface) -> vec4f`），不过时返回可读错误。
    pub fn check(code: &str) -> Result<(), String> {
        let base = crate::renderer::shader_source(true);
        let full = compose(&base, code);
        validate(&full, code)
    }
}

/// 挂在 3D 模型或网格实体上，使用 [`Shaders3d`] 里注册的自定义 3D 着色器与 4 个浮点参数。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct MaterialShader3d {
    pub name: String,
    pub params: Vec4,
}

/// 注入到主 3D 着色器里的结构体与默认钩子标记。
pub(crate) const CUSTOM_HOOK_MARKER: &str = "// NGE_CUSTOM_SHADE3D_HOOK";
pub(crate) const CUSTOM_CALL_MARKER: &str = "// NGE_CUSTOM_SHADE3D_CALL";

/// 主 3D 着色器模板声明的绑定总数（group 0: 8, group 1: 4, group 2: 6）。
const BASE_BINDINGS: usize = 18;

const SURFACE_STRUCT: &str = r#"
/// 传给 `fn shade3d(s: Surface) -> vec4f` 的片元上下文。
struct Surface {
    /// 已经算完 PBR / 光照 / 阴影 / 雾的结果（rgb 线性亮度，a 透明度）。
    color: vec4f,
    /// 材质基础色（贴图 × base_color）。
    albedo: vec4f,
    /// 片元世界坐标。
    world: vec3f,
    /// 世界空间法线（已含法线贴图）。
    normal: vec3f,
    /// 指向相机的单位视线向量。
    view: vec3f,
    /// 贴图坐标。
    uv: vec2f,
    /// 来自实体 `params` 的 4 个自定义参数。
    params: vec4f,
    /// 经过的秒数。
    time: f32,
}
"#;

const CALL_SNIPPET: &str = r#"
    var nge_surf: Surface;
    nge_surf.color = vec4f(color, select(1.0, base.a, d.emissive.w > 0.0));
    nge_surf.albedo = base;
    nge_surf.world = i.world;
    nge_surf.normal = n;
    nge_surf.view = v;
    nge_surf.uv = i.uv;
    nge_surf.params = d.custom;
    nge_surf.time = g.ambient.w;
    let nge_custom = shade3d(nge_surf);
    color = nge_custom.rgb;
    out_alpha = nge_custom.a;
"#;

/// 把用户的 `shade3d` 拼进主 3D 着色器源码。
pub(crate) fn compose(base_wgsl: &str, custom_fn: &str) -> String {
    let hook = format!("{SURFACE_STRUCT}\n{custom_fn}\n");
    base_wgsl.replace(CUSTOM_HOOK_MARKER, &hook).replace(CUSTOM_CALL_MARKER, CALL_SNIPPET)
}

/// 用 naga 解析、校验并拒绝用户自己声明额外 `@group/@binding`、`override` 或 `var<immediate>`。
pub(crate) fn validate(source: &str, user: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| shift(&e.emit_to_string(source), source, user))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
        .validate(&module)
        .map_err(|e| shift(&e.emit_to_string(source), source, user))?;
    let bindings = module.global_variables.iter().filter(|(_, g)| g.binding.is_some()).count();
    let immediate = module.global_variables.iter().any(|(_, g)| g.space == naga::AddressSpace::Immediate);
    if bindings != BASE_BINDINGS || immediate || !module.overrides.is_empty() {
        return Err(shift("不能自己声明绑定（@group / @binding）、override 或 var<immediate>：参数用 params 传", source, user));
    }
    Ok(())
}

fn shift(message: &str, source: &str, user: &str) -> String {
    let before = source.find(user).map_or(0, |at| source[..at].lines().count());
    format!("{message}\n（游戏写的 3D 着色器从完整着色器的第 {} 行开始）", before + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_custom_3d_shader_passes_and_invalid_ones_are_rejected() {
        let shield = r#"
fn shade3d(s: Surface) -> vec4f {
    let fresnel = pow(1.0 - max(dot(s.normal, s.view), 0.0), 3.0);
    let pulse = 0.6 + 0.4 * sin(s.world.y * 0.2 - s.time * 4.0);
    return vec4f(s.color.rgb + s.params.rgb * fresnel * pulse, s.color.a);
}
"#;
        assert!(Shaders3d::check(shield).is_ok(), "{:?}", Shaders3d::check(shield));
        assert!(Shaders3d::check("fn wrong() {}").is_err());
        assert!(Shaders3d::check("fn shade3d(s: Surface) -> f32 { return 1.0; }").is_err());
        let bad_binding = "@group(3) @binding(0) var<uniform> u: vec4f;\nfn shade3d(s: Surface) -> vec4f { return s.color * u; }";
        assert!(Shaders3d::check(bad_binding).unwrap_err().contains("不能自己声明绑定"));
    }

    #[test]
    fn shaders3d_generation_bumps_only_on_change() {
        let mut lib = Shaders3d::default();
        let map = HashMap::from([("holo".to_owned(), "fn shade3d(s: Surface) -> vec4f { return s.color; }".to_owned())]);
        assert!(lib.set(map.clone()));
        assert_eq!(lib.generation(), 1);
        assert!(!lib.set(map));
        assert_eq!(lib.generation(), 1);
    }
}
