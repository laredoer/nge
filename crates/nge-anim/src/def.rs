//! 状态机的声明（和 TSX 的 `AnimGraphDef` 一一对应，见 docs/anim-graph.md）和编译：名字换成下标，顺带查错。

use std::collections::BTreeMap;

use serde::Deserialize;

/// 整个状态机。没有 `layers` 时顶层的 `start` / `states` / `any` 是唯一的一层；有 `layers` 时顶层的是第 0 层（可以不写）。
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GraphDef {
    /// 参数和初始值：数是 float，true / false 是 bool，`"trigger"` 是 trigger。
    pub params: BTreeMap<String, ParamDef>,
    /// 片段的事件；2D 的片段在这里写帧。
    pub clips: BTreeMap<String, ClipDef>,
    pub start: Option<String>,
    pub states: BTreeMap<String, StateDef>,
    pub any: Vec<TransitionDef>,
    pub layers: Vec<LayerDef>,
}

impl GraphDef {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("状态机定义不是合法的 JSON：{e}"))
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ParamDef {
    Float(f32),
    Bool(bool),
    /// 只能是 "trigger"。
    Kind(String),
}

/// 游戏设参数时给的值（TSX 的 `animParams` / `animSet`）：数设 float，true / false 设 bool。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Float(f32),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipDef {
    /// 2D：依次播的图集格子。
    pub frames: Vec<u32>,
    pub fps: f32,
    pub events: Vec<EventDef>,
}

impl Default for ClipDef {
    fn default() -> Self {
        Self { frames: Vec::new(), fps: 10.0, events: Vec::new() }
    }
}

/// 片段播到 `t` 秒（片段自己的时间）时发 `name`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDef {
    pub t: f32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct StateDef {
    pub clip: Option<String>,
    pub blend: Option<BlendDef>,
    pub speed: f32,
    /// 速度再乘上这个 float 参数。
    pub speed_param: Option<String>,
    #[serde(rename = "loop")]
    pub looping: bool,
    pub to: Vec<TransitionDef>,
}

impl Default for StateDef {
    fn default() -> Self {
        Self { clip: None, blend: None, speed: 1.0, speed_param: None, looping: true, to: Vec::new() }
    }
}

/// 混合树：`by` 一个参数名是一维，两个是二维。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlendDef {
    pub by: OneOrMany,
    pub clips: Vec<BlendClipDef>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlendClipDef {
    pub clip: String,
    pub at: AtDef,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum AtDef {
    One(f32),
    Two([f32; 2]),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl Default for OneOrMany {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl OneOrMany {
    fn list(&self) -> Vec<&str> {
        match self {
            Self::One(s) => vec![s.as_str()],
            Self::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransitionDef {
    pub to: String,
    /// 条件，全部成立才转。
    pub when: OneOrMany,
    /// 过渡秒数，0 是硬切。
    pub fade: f32,
    /// 退出时间（归一化，1 是播完一遍）。
    pub exit: Option<f32>,
}

impl Default for TransitionDef {
    fn default() -> Self {
        Self { to: String::new(), when: OneOrMany::default(), fade: 0.0, exit: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerMode {
    #[default]
    Override,
    Additive,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum WeightDef {
    Fixed(f32),
    Param(String),
}

impl Default for WeightDef {
    fn default() -> Self {
        Self::Fixed(1.0)
    }
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerDef {
    pub name: String,
    pub weight: WeightDef,
    pub mode: LayerMode,
    /// 骨骼遮罩：节点名，含子孙。不写是全身。
    pub mask: Option<Vec<String>>,
    /// 叠加层的参考姿势：这个片段的 0 秒；不写时每个片段用自己的 0 秒。
    pub reference: Option<String>,
    pub start: Option<String>,
    pub states: BTreeMap<String, StateDef>,
    pub any: Vec<TransitionDef>,
}

// ---------- 编译后 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Float,
    Bool,
    Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Value {
    Float(f32),
    Bool(bool),
    Trigger(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Cond {
    /// bool 为真，或 trigger 被设了。
    True(usize),
    False(usize),
    Cmp(usize, Op, f32),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Transition {
    pub to: usize,
    pub when: Vec<Cond>,
    pub fade: f32,
    pub exit: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Motion {
    Clip(usize),
    /// `by` 一个或两个参数；`at` 一维时只用第 0 个分量。
    Blend { by: Vec<usize>, clips: Vec<usize>, at: Vec<[f32; 2]> },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct State {
    pub name: String,
    pub motion: Motion,
    pub speed: f32,
    pub speed_param: Option<usize>,
    pub looping: bool,
    pub to: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Weight {
    Fixed(f32),
    Param(usize),
}

/// 编译好的一层。
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub name: String,
    pub additive: bool,
    pub mask: Option<Vec<String>>,
    /// 叠加层参考姿势的片段（下标）。
    pub reference: Option<usize>,
    pub(crate) weight: Weight,
    pub(crate) start: usize,
    pub(crate) states: Vec<State>,
    pub(crate) any: Vec<Transition>,
}

/// 一个片段：名字、事件（按时间排好）、2D 的帧。
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub name: String,
    pub events: Vec<(f32, String)>,
    pub frames: Vec<u32>,
    pub fps: f32,
}

impl Clip {
    /// 2D 片段的时长（帧数 / fps）；没写帧时是 `None`。
    pub fn frames_duration(&self) -> Option<f32> {
        (!self.frames.is_empty()).then(|| self.frames.len() as f32 / self.fps)
    }

    /// 2D：`time` 秒时画哪一格（夹在最后一格）。
    pub fn frame_at(&self, time: f32) -> Option<u32> {
        let last = self.frames.len().checked_sub(1)?;
        let i = (time * self.fps).floor();
        let i = if i.is_finite() && i > 0.0 { (i as usize).min(last) } else { 0 };
        Some(self.frames[i])
    }
}

/// 编译好的状态机，多个实体可以共用（`Arc`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Graph {
    pub(crate) params: Vec<(String, ParamKind)>,
    pub(crate) defaults: Vec<Value>,
    pub(crate) clips: Vec<Clip>,
    pub(crate) layers: Vec<Layer>,
}

impl Graph {
    pub fn compile(def: &GraphDef) -> Result<Self, String> {
        let mut params = Vec::new();
        let mut defaults = Vec::new();
        for (name, p) in &def.params {
            let (kind, value) = match p {
                ParamDef::Float(v) if v.is_finite() => (ParamKind::Float, Value::Float(*v)),
                ParamDef::Float(_) => return Err(format!("参数 {name:?} 的初始值不是有限数")),
                ParamDef::Bool(b) => (ParamKind::Bool, Value::Bool(*b)),
                ParamDef::Kind(k) if k == "trigger" => (ParamKind::Trigger, Value::Trigger(false)),
                ParamDef::Kind(k) => return Err(format!("参数 {name:?} 的初始值 {k:?} 不对：写数、true / false，或 \"trigger\"")),
            };
            params.push((name.clone(), kind));
            defaults.push(value);
        }
        let mut b = Builder { params: &params, clips: Vec::new() };
        for (name, c) in &def.clips {
            let i = b.clip(name);
            if !c.frames.is_empty() && !(c.fps.is_finite() && c.fps > 0.0) {
                return Err(format!("片段 {name:?} 的 fps 要是正数"));
            }
            let mut events = Vec::new();
            for e in &c.events {
                if !(e.t.is_finite() && e.t >= 0.0) {
                    return Err(format!("片段 {name:?} 的事件 {:?} 的时间要是不小于 0 的数", e.name));
                }
                events.push((e.t, e.name.clone()));
            }
            events.sort_by(|a, b| a.0.total_cmp(&b.0));
            b.clips[i] = Clip { name: name.clone(), events, frames: c.frames.clone(), fps: c.fps };
        }
        let mut layers = Vec::new();
        let top = (!def.states.is_empty() || def.start.is_some() || !def.any.is_empty())
            .then(|| LayerDef { name: "base".into(), start: def.start.clone(), states: def.states.clone(), any: def.any.clone(), ..LayerDef::default() });
        for (i, layer) in top.iter().chain(&def.layers).enumerate() {
            let name = if layer.name.is_empty() { format!("{i}") } else { layer.name.clone() };
            if layers.iter().any(|l: &Layer| l.name == name) {
                return Err(format!("层名重复：{name:?}（只有顶层的 states 时第 0 层叫 \"base\"）"));
            }
            // 第 0 层下面没东西：权重固定是 1，3D 姿势、2D 选帧、事件都按 1 算
            if i == 0 && layer.weight != WeightDef::Fixed(1.0) {
                return Err(format!("第 0 层 {name:?} 的 weight 固定是 1，不能写别的数或参数"));
            }
            layers.push(b.layer(layer, name)?);
        }
        if layers.is_empty() {
            return Err("状态机没有状态".into());
        }
        let clips = b.clips;
        Ok(Self { params, defaults, clips, layers })
    }

    pub fn params(&self) -> impl Iterator<Item = (&str, ParamKind)> {
        self.params.iter().map(|(n, k)| (n.as_str(), *k))
    }

    pub fn param(&self, name: &str) -> Option<(usize, ParamKind)> {
        self.params.iter().position(|(n, _)| n == name).map(|i| (i, self.params[i].1))
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn clip(&self, name: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.name == name)
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// 第 `layer` 层所有状态的名字。
    pub fn states(&self, layer: usize) -> Vec<&str> {
        self.layers.get(layer).map(|l| l.states.iter().map(|s| s.name.as_str()).collect()).unwrap_or_default()
    }
}

struct Builder<'a> {
    params: &'a [(String, ParamKind)],
    clips: Vec<Clip>,
}

impl Builder<'_> {
    fn clip(&mut self, name: &str) -> usize {
        match self.clips.iter().position(|c| c.name == name) {
            Some(i) => i,
            None => {
                self.clips.push(Clip { name: name.to_owned(), events: Vec::new(), frames: Vec::new(), fps: 10.0 });
                self.clips.len() - 1
            }
        }
    }

    fn param(&self, name: &str, want: &[ParamKind], what: &str) -> Result<usize, String> {
        match self.params.iter().position(|(n, _)| n == name) {
            None => Err(format!("{what}：没有参数 {name:?}")),
            Some(i) if want.contains(&self.params[i].1) => Ok(i),
            Some(i) => Err(format!("{what}：参数 {name:?} 是 {:?}，这里要 {want:?}", self.params[i].1)),
        }
    }

    fn layer(&mut self, def: &LayerDef, name: String) -> Result<Layer, String> {
        let names: Vec<&String> = def.states.keys().collect();
        let index = |s: &str, what: &str| names.iter().position(|n| *n == s).ok_or_else(|| format!("层 {name:?}{what}：没有状态 {s:?}"));
        let start = match &def.start {
            Some(s) => index(s, " 的 start")?,
            None if names.len() == 1 => 0,
            None => return Err(format!("层 {name:?} 有好几个状态，要写 start")),
        };
        let weight = match &def.weight {
            WeightDef::Fixed(w) if w.is_finite() => Weight::Fixed(w.clamp(0.0, 1.0)),
            WeightDef::Fixed(_) => return Err(format!("层 {name:?} 的 weight 不是有限数")),
            WeightDef::Param(p) => Weight::Param(self.param(p, &[ParamKind::Float], &format!("层 {name:?} 的 weight"))?),
        };
        let transitions = |list: &[TransitionDef], what: &str| -> Result<Vec<Transition>, String> {
            list.iter()
                .map(|t| {
                    let what = format!("层 {name:?} {what} 到 {:?} 的转换", t.to);
                    let to = index(&t.to, &format!(" {what}"))?;
                    let when = t.when.list().iter().map(|c| self.condition(c, &what)).collect::<Result<_, _>>()?;
                    if !t.fade.is_finite() || t.exit.is_some_and(|e| !e.is_finite()) {
                        return Err(format!("{what}：fade、exit 要是有限数"));
                    }
                    if t.when.list().is_empty() && t.exit.is_none() {
                        return Err(format!("{what}：没有条件也没有 exit，每一步都会转"));
                    }
                    Ok(Transition { to, when, fade: t.fade.max(0.0), exit: t.exit })
                })
                .collect()
        };
        let any = transitions(&def.any, "any")?;
        let tos = def.states.iter().map(|(n, s)| transitions(&s.to, &format!("状态 {n:?}"))).collect::<Result<Vec<_>, _>>()?;
        let mut states = Vec::new();
        for ((state_name, s), to) in def.states.iter().zip(tos) {
            let what = format!("层 {name:?} 状态 {state_name:?}");
            let motion = match (&s.clip, &s.blend) {
                (Some(c), None) => Motion::Clip(self.clip(c)),
                (None, Some(b)) => self.blend(b, &what)?,
                _ => return Err(format!("{what}：clip 和 blend 要写一个且只写一个")),
            };
            if !s.speed.is_finite() {
                return Err(format!("{what}：speed 不是有限数"));
            }
            let speed_param = s.speed_param.as_deref().map(|p| self.param(p, &[ParamKind::Float], &format!("{what} 的 speedParam"))).transpose()?;
            states.push(State { name: state_name.clone(), motion, speed: s.speed, speed_param, looping: s.looping, to });
        }
        let reference = def.reference.as_deref().map(|c| self.clip(c));
        Ok(Layer { name, additive: def.mode == LayerMode::Additive, mask: def.mask.clone(), reference, weight, start, states, any })
    }

    fn blend(&mut self, b: &BlendDef, what: &str) -> Result<Motion, String> {
        let by = b.by.list();
        if !(1..=2).contains(&by.len()) {
            return Err(format!("{what}：blend.by 要一个（一维）或两个（二维）参数"));
        }
        let by = by.iter().map(|p| self.param(p, &[ParamKind::Float], &format!("{what} 的 blend"))).collect::<Result<Vec<_>, _>>()?;
        if b.clips.is_empty() {
            return Err(format!("{what}：blend 里没有片段"));
        }
        let mut clips = Vec::new();
        let mut at = Vec::new();
        for c in &b.clips {
            let p = match (&c.at, by.len()) {
                (AtDef::One(x), 1) => [*x, 0.0],
                (AtDef::Two(p), 2) => *p,
                _ => return Err(format!("{what}：片段 {:?} 的 at 要和 by 的个数一样（一维写数，二维写 [x, y]）", c.clip)),
            };
            if !(p[0].is_finite() && p[1].is_finite()) {
                return Err(format!("{what}：片段 {:?} 的 at 不是有限数", c.clip));
            }
            if at.contains(&p) {
                return Err(format!("{what}：片段 {:?} 的 at 和前面的位置重复", c.clip));
            }
            clips.push(self.clip(&c.clip));
            at.push(p);
        }
        Ok(Motion::Blend { by, clips, at })
    }

    fn condition(&self, text: &str, what: &str) -> Result<Cond, String> {
        let what = format!("{what} 的条件 {text:?}");
        let text = text.trim();
        if let Some(name) = text.strip_prefix('!') {
            return Ok(Cond::False(self.param(name.trim(), &[ParamKind::Bool], &what)?));
        }
        const OPS: [(&str, Op); 6] = [(">=", Op::Ge), ("<=", Op::Le), ("==", Op::Eq), ("!=", Op::Ne), (">", Op::Gt), ("<", Op::Lt)];
        for (sym, op) in OPS {
            if let Some((l, r)) = text.split_once(sym) {
                let v: f32 = r.trim().parse().map_err(|_| format!("{what}：右边要是数"))?;
                if !v.is_finite() {
                    return Err(format!("{what}：右边要是有限数"));
                }
                return Ok(Cond::Cmp(self.param(l.trim(), &[ParamKind::Float], &what)?, op, v));
            }
        }
        Ok(Cond::True(self.param(text, &[ParamKind::Bool, ParamKind::Trigger], &what)?))
    }
}
