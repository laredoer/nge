//! 场景 JSON 宽松解析：一项写错只丢那一项（warn 带字段名），不让整个场景解析失败（见 docs/gameplay3d.md「场景里写错一项」）。
//!
//! 通用规则：`Scene` 的每个列表逐项解析、每个可选项单独解析，写错的那一项 warn 后跳过（可选项当没写）；
//! 节点里嵌着的列表、可选项也一样。会超出范围的数（碰撞分组）收宽的类型再查范围，超出的 warn 后按默认处理。
//! `sky`、`fog`、`shadows`（见 docs/render3d-quality.md）和灯的 `shadowPriority` 再细一层：逐字段解析，写错的字段 warn 后当没写，别的字段照用。

use std::collections::HashSet;
use std::sync::Mutex;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// 记着说过的话，同一句只说一次（场景每次变化都重新解析）。太多时清掉重来。
static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
const MAX_WARNED: usize = 1024;

/// 没说过这句时 warn，返回说没说。
pub(crate) fn warn_once(message: String) -> bool {
    let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    let set = warned.get_or_insert_with(HashSet::new);
    if set.len() >= MAX_WARNED {
        set.clear();
    }
    let new = set.insert(message.clone());
    if new {
        log::warn!("{message}");
    }
    new
}

/// 一项的名字：有 `id` 用 id，没有用下标。
fn label(value: &Value, index: usize) -> String {
    match value.get("id") {
        Some(Value::String(id)) => format!("{id:?}"),
        Some(id) => id.to_string(),
        None => format!("第 {index} 项"),
    }
}

/// 列表逐项解析：解析不了的那一项 warn 后跳过；整个不是数组时 warn 后当空的。
pub(crate) fn items<T: DeserializeOwned>(field: &str, value: Value) -> Vec<T> {
    let list = match value {
        Value::Null => return Vec::new(),
        Value::Array(list) => list,
        other => {
            warn_once(format!("场景的 {field} 要是数组，收到 {other}，当没写"));
            return Vec::new();
        }
    };
    list.into_iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let name = label(&v, i);
            T::deserialize(v).map_err(|e| warn_once(format!("场景的 {field} 里 {name} 写错了，跳过这一项：{e}"))).ok()
        })
        .collect()
}

/// 可选项单独解析：解析不了 warn 后当没写。
pub(crate) fn optional<T: DeserializeOwned>(field: &str, value: Value) -> Option<T> {
    if value.is_null() {
        return None;
    }
    T::deserialize(value).map_err(|e| warn_once(format!("场景的 {field} 写错了，当没写：{e}"))).ok()
}

/// 碰撞分组：0 … 31，超出的 warn 后当不分组（0）。
pub(crate) fn group(field: &str, g: i64) -> u8 {
    match u8::try_from(g) {
        Ok(g) if g < 32 => g,
        _ => {
            warn_once(format!("{field} 的碰撞分组 {g} 超出 0 … 31，当不分组"));
            0
        }
    }
}

/// 给 `#[serde(deserialize_with = ...)]` 用的：按字段名生成逐项 / 单独解析的函数。
macro_rules! lenient {
    ($($list:ident => $lname:literal),* ; $($opt:ident => $oname:literal),*) => {
        $(
            pub(crate) fn $list<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Vec<T>, D::Error> {
                Ok(items($lname, Value::deserialize(d)?))
            }
        )*
        $(
            pub(crate) fn $opt<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<Option<T>, D::Error> {
                Ok(optional($oname, Value::deserialize(d)?))
            }
        )*
    };
}

lenient!(
    sprites => "sprites", tilemaps => "tilemaps", texts => "texts", emitters => "emitters", models => "models",
    emitters3d => "emitters3d", decals3d => "decals3d", lights => "lights", bars => "bars", bubbles => "bubbles", joints => "joints",
    joints3d => "joints3d", post => "post", lighting_lights => "lighting.lights", tile_slopes => "tilemaps.slopes",
    tile_animated => "tilemaps.animated", tile_materials => "tilemaps.materials", instances => "instances",
    set_tiles => "setTiles.tiles", audio_effects => "audio.buses 的 effects", audio_emitters => "audio.emitters";
    background => "background", camera => "camera", camera3d => "camera3d", ambient => "ambient", physics => "physics",
    physics3d => "physics3d", lighting => "lighting", audio => "audio", time_scale => "timeScale", perf => "perf",
    net => "net", bounds => "camera.bounds", behavior => "sprites.behavior", transition => "transition",
    sky => "sky", fog => "fog", shadows => "shadows", tonemap => "tonemap", ssao => "ssao", navmesh3d => "navmesh3d", shadow_priority => "lights.shadowPriority", tile_chunk => "tilemaps.chunk"
);

/// 逐字段解析的对象：`get` 一个字段，写错的 warn 后当没写。
pub(crate) struct Fields {
    name: &'static str,
    map: serde_json::Map<String, Value>,
}

impl Fields {
    /// 不是对象时报错（整项当没写）。
    pub(crate) fn new(name: &'static str, value: Value) -> Result<Self, String> {
        match value {
            Value::Object(map) => Ok(Self { name, map }),
            other => Err(format!("要是对象，收到 {other}")),
        }
    }

    pub(crate) fn get<T: DeserializeOwned>(&mut self, key: &str) -> Option<T> {
        optional(&format!("{}.{key}", self.name), self.map.remove(key).unwrap_or(Value::Null))
    }

    /// 写了但解析不了时返回错误（不 warn，让整项当没写）。
    pub(crate) fn strict<T: DeserializeOwned>(&mut self, key: &str) -> Result<Option<T>, String> {
        match self.map.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => T::deserialize(v).map(Some).map_err(|e| format!("{key} 写错了：{e}")),
        }
    }

    /// 剩下没认出来的字段各 warn 一次（多半是拼错了）。
    pub(crate) fn finish(self) {
        for key in self.map.keys() {
            warn_once(format!("场景的 {} 里没有 {key:?} 这个字段，忽略", self.name));
        }
    }
}

/// 着色器表：值不是字符串的那一个跳过。
pub(crate) fn shaders<'de, D: Deserializer<'de>>(d: D) -> Result<std::collections::HashMap<String, String>, D::Error> {
    let map = match Value::deserialize(d)? {
        Value::Null => return Ok(Default::default()),
        Value::Object(map) => map,
        other => {
            warn_once(format!("场景的 shaders 要是对象，收到 {other}，当没写"));
            return Ok(Default::default());
        }
    };
    Ok(map
        .into_iter()
        .filter_map(|(name, v)| match v {
            Value::String(code) => Some((name, code)),
            other => {
                warn_once(format!("场景的 shaders 里 {name:?} 要是字符串，收到 {other}，跳过"));
                None
            }
        })
        .collect())
}

/// 名字 → 对象的表：值解析不了的那一个跳过。
fn map<T: DeserializeOwned>(field: &str, value: Value) -> std::collections::HashMap<String, T> {
    let map = match value {
        Value::Null => return Default::default(),
        Value::Object(map) => map,
        other => {
            warn_once(format!("场景的 {field} 要是对象，收到 {other}，当没写"));
            return Default::default();
        }
    };
    map.into_iter()
        .filter_map(|(name, v)| T::deserialize(v).map(|t| (name.clone(), t)).map_err(|e| warn_once(format!("场景的 {field} 里 {name:?} 写错了，跳过：{e}"))).ok())
        .collect()
}

/// 预制体表：不是对象的那一个跳过。
pub(crate) fn prefabs<'de, D: Deserializer<'de>>(d: D) -> Result<std::collections::HashMap<String, Value>, D::Error> {
    Ok(map::<serde_json::Map<String, Value>>("prefabs", Value::deserialize(d)?).into_iter().map(|(k, v)| (k, Value::Object(v))).collect())
}

pub(crate) fn textures<'de, D: Deserializer<'de>, T: DeserializeOwned>(d: D) -> Result<std::collections::HashMap<String, T>, D::Error> {
    Ok(map("textures", Value::deserialize(d)?))
}
