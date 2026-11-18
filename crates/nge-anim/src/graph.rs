//! 状态机的运行状态：参数、每层的状态实例、过渡，以及每步的推进（见 docs/anim-graph.md「求值」）。

use std::sync::Arc;

use bevy_ecs::prelude::*;

use crate::blend::{blend1d, blend2d};
use crate::def::{Cond, Graph, GraphDef, Layer, Motion, Op, ParamKind, ParamValue, State, Transition, Value, Weight};
use crate::events::crossed;

/// 一层最多同时有几个状态实例（连续打断的过渡），多了丢掉最轻的。
pub const MAX_INSTANCES: usize = 8;

/// 退出时间比较时的余量：不循环的片段一步步加起来可能差一点点到不了 1。
const EXIT_SLACK: f64 = 1e-5;

/// 这一步触发的事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Fired {
    pub layer: String,
    pub state: String,
    pub clip: String,
    pub name: String,
}

/// 一层这一帧在播的片段。
#[derive(Debug, Clone, PartialEq)]
pub struct ClipSample {
    /// 片段下标（`Graph::clips`）。
    pub clip: usize,
    /// 采样时间（秒）：循环的取过模，不循环的夹在头尾。
    pub time: f32,
    /// 在这一层里的权重，同一层加起来是 1。
    pub weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerOutput {
    pub layer: usize,
    /// 层权重（0–1）。
    pub weight: f32,
    pub additive: bool,
    pub clips: Vec<ClipSample>,
}

/// 一个活着的状态实例。
#[derive(Debug, Clone, PartialEq)]
struct Instance {
    state: usize,
    /// 归一化相位：循环的不封顶（取小数部分采样），不循环的夹在 [0, 1]。
    phase: f64,
    /// 进入以后实际播过的归一化长度（退出时间用）。
    played: f64,
    /// 还没走过第一步：起点上的事件也算。
    fresh: bool,
    /// 这次过渡开始时的权重。
    from: f32,
    weight: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct LayerRun {
    instances: Vec<Instance>,
    /// 现在的（转换中是目标）状态在 `instances` 里的下标。
    current: usize,
    /// 过渡进度 0–1 和时长。
    progress: f32,
    fade: f32,
}

/// 状态机组件：编译好的图（同一份定义的实体共用）、参数、每层的运行状态、这一帧的输出。
#[derive(Component, Debug, Clone)]
pub struct AnimGraph {
    graph: Arc<Graph>,
    params: Vec<Value>,
    layers: Vec<LayerRun>,
    out: Vec<LayerOutput>,
}

impl AnimGraph {
    pub fn new(graph: Arc<Graph>) -> Self {
        let params = graph.defaults.clone();
        let layers = graph
            .layers
            .iter()
            .map(|l| LayerRun { instances: vec![Instance::enter(l.start, start_phase(&params, &l.states[l.start]), 1.0)], current: 0, progress: 1.0, fade: 0.0 })
            .collect();
        Self { graph, params, layers, out: Vec::new() }
    }

    pub fn from_def(def: &GraphDef) -> Result<Self, String> {
        Ok(Self::new(Arc::new(Graph::compile(def)?)))
    }

    pub fn graph(&self) -> &Arc<Graph> {
        &self.graph
    }

    fn slot(&mut self, name: &str, kind: ParamKind) -> Option<&mut Value> {
        let (i, k) = self.graph.param(name)?;
        (k == kind).then(|| &mut self.params[i])
    }

    fn value(&self, name: &str, kind: ParamKind) -> Option<Value> {
        let (i, k) = self.graph.param(name)?;
        (k == kind).then_some(self.params[i])
    }

    /// 设 float 参数；没有这个名字、不是 float、不是有限数时返回 false。
    pub fn set_float(&mut self, name: &str, v: f32) -> bool {
        match self.slot(name, ParamKind::Float) {
            Some(slot) if v.is_finite() => {
                *slot = Value::Float(v);
                true
            }
            _ => false,
        }
    }

    pub fn set_bool(&mut self, name: &str, v: bool) -> bool {
        self.slot(name, ParamKind::Bool).map(|slot| *slot = Value::Bool(v)).is_some()
    }

    /// 按值的类型设 float 或 bool 参数。
    pub fn set(&mut self, name: &str, value: ParamValue) -> bool {
        match value {
            ParamValue::Float(v) => self.set_float(name, v),
            ParamValue::Bool(v) => self.set_bool(name, v),
        }
    }

    /// 设 trigger：留着，直到某条转换用掉它。
    pub fn trigger(&mut self, name: &str) -> bool {
        self.slot(name, ParamKind::Trigger).map(|slot| *slot = Value::Trigger(true)).is_some()
    }

    pub fn reset_trigger(&mut self, name: &str) -> bool {
        self.slot(name, ParamKind::Trigger).map(|slot| *slot = Value::Trigger(false)).is_some()
    }

    pub fn float(&self, name: &str) -> Option<f32> {
        match self.value(name, ParamKind::Float)? {
            Value::Float(v) => Some(v),
            _ => None,
        }
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.value(name, ParamKind::Bool)? {
            Value::Bool(v) => Some(v),
            _ => None,
        }
    }

    pub fn triggered(&self, name: &str) -> Option<bool> {
        match self.value(name, ParamKind::Trigger)? {
            Value::Trigger(v) => Some(v),
            _ => None,
        }
    }

    /// 第 `layer` 层现在在哪个状态（转换中是目标状态）。
    pub fn state(&self, layer: usize) -> Option<&str> {
        let run = self.layers.get(layer)?;
        Some(&self.graph.layers[layer].states[run.instances[run.current].state].name)
    }

    /// 往前走 `dt` 秒（不是有限的正数时当 0）：推进相位、发事件、看转换、算输出。`duration` 是片段名 → 时长（秒）。
    pub fn step(&mut self, dt: f32, duration: impl Fn(&str) -> Option<f32>) -> Vec<Fired> {
        let dt = if dt.is_finite() && dt > 0.0 { f64::from(dt) } else { 0.0 };
        let graph = Arc::clone(&self.graph);
        let durations: Vec<f32> = graph.clips.iter().map(|c| duration(&c.name).filter(|d| d.is_finite() && *d > 0.0).unwrap_or(0.0)).collect();
        let mut fired = Vec::new();
        self.out.clear();
        for (li, layer) in graph.layers.iter().enumerate() {
            let weight = layer_weight(&self.params, layer);
            let run = &mut self.layers[li];
            run.fade_step(dt);
            advance(&graph, layer, run, &self.params, &durations, dt, weight > 0.0, &mut fired);
            if let Some(t) = pick(layer, run, &self.params) {
                for c in &t.when {
                    if let Cond::True(p) = c
                        && let Value::Trigger(_) = self.params[*p]
                    {
                        self.params[*p] = Value::Trigger(false);
                    }
                }
                let target = &layer.states[t.to];
                let phase = start_phase(&self.params, target);
                if self.layers[li].begin(t.to, t.fade, phase, target.looping) && weight > 0.0 {
                    let run = &mut self.layers[li];
                    let inst = &mut run.instances[run.current];
                    entry_events(&graph, layer, inst, &self.params, &durations, &mut fired);
                }
            }
            let clips = output(layer, &self.layers[li], &self.params, &durations);
            self.out.push(LayerOutput { layer: li, weight, additive: layer.additive, clips });
        }
        self.drop_unusable_triggers();
        fired
    }

    /// 现在的状态（每层）没有哪条转换用得上的 trigger 清掉，免得很久以后误触发。
    fn drop_unusable_triggers(&mut self) {
        for p in 0..self.params.len() {
            if self.params[p] != Value::Trigger(true) {
                continue;
            }
            let usable = self.graph.layers.iter().zip(&self.layers).any(|(layer, run)| {
                let here = run.instances[run.current].state;
                let uses = |t: &Transition| t.when.contains(&Cond::True(p));
                layer.any.iter().filter(|t| t.to != here).any(uses) || layer.states[here].to.iter().any(uses)
            });
            if !usable {
                self.params[p] = Value::Trigger(false);
            }
        }
    }

    /// 回到默认状态和参数的初始值（重生、重开时用）。
    pub fn reset(&mut self) {
        *self = Self::new(Arc::clone(&self.graph));
    }

    /// 这一帧每层在播的片段；第一次 [`step`](Self::step) 之前是空的。
    pub fn output(&self) -> &[LayerOutput] {
        &self.out
    }
}

impl Instance {
    fn enter(state: usize, phase: f64, weight: f32) -> Self {
        Self { state, phase, played: 0.0, fresh: true, from: weight, weight }
    }
}

impl LayerRun {
    /// 过渡往前走；走完只留目标。
    fn fade_step(&mut self, dt: f64) {
        if self.instances.len() == 1 {
            return;
        }
        self.progress = if self.fade > 0.0 { (self.progress + (dt / f64::from(self.fade)) as f32).min(1.0) } else { 1.0 };
        if self.progress >= 1.0 {
            self.keep_only_current();
            return;
        }
        let s = self.progress;
        for (k, inst) in self.instances.iter_mut().enumerate() {
            inst.weight = if k == self.current { inst.from + (1.0 - inst.from) * s } else { inst.from * (1.0 - s) };
        }
    }

    fn keep_only_current(&mut self) {
        let mut inst = self.instances.swap_remove(self.current);
        (inst.from, inst.weight) = (1.0, 1.0);
        self.instances = vec![inst];
        self.current = 0;
        self.progress = 1.0;
    }

    /// 开始转到 `to`：现在的权重当起点。还在淡出的同一个循环状态接着用；不循环的、转到自己的从头开始（新实例）。
    /// 返回是不是新进入的实例。
    fn begin(&mut self, to: usize, fade: f32, phase: f64, looping: bool) -> bool {
        for inst in &mut self.instances {
            inst.from = inst.weight;
        }
        let current = self.current;
        let reuse = if looping { self.instances.iter().enumerate().position(|(k, i)| k != current && i.state == to) } else { None };
        let fresh = reuse.is_none();
        self.current = match reuse {
            Some(k) => k,
            None => {
                self.instances.push(Instance::enter(to, phase, 0.0));
                self.instances.len() - 1
            }
        };
        (self.progress, self.fade) = (0.0, fade);
        if fade <= 0.0 {
            self.keep_only_current();
            return fresh;
        }
        while self.instances.len() > crate::MAX_INSTANCES {
            let lightest = (0..self.instances.len()).filter(|&k| k != self.current).min_by(|&a, &b| self.instances[a].weight.total_cmp(&self.instances[b].weight)).expect("不止一个");
            self.instances.remove(lightest);
            if lightest < self.current {
                self.current -= 1;
            }
            let others: f32 = self.instances.iter().enumerate().filter(|(k, _)| *k != self.current).map(|(_, i)| i.from).sum();
            let cur = self.instances[self.current].from;
            let total = cur + others;
            if total > 0.0 {
                for inst in &mut self.instances {
                    inst.from /= total;
                    inst.weight = inst.from;
                }
            }
        }
        fresh
    }
}

fn float(params: &[Value], p: usize) -> f32 {
    match params[p] {
        Value::Float(v) => v,
        _ => 0.0,
    }
}

fn layer_weight(params: &[Value], layer: &Layer) -> f32 {
    let w = match layer.weight {
        Weight::Fixed(w) => w,
        Weight::Param(p) => float(params, p),
    };
    if w.is_nan() { 0.0 } else { w.clamp(0.0, 1.0) }
}

fn speed(params: &[Value], state: &State) -> f64 {
    f64::from(state.speed * state.speed_param.map_or(1.0, |p| float(params, p)))
}

/// 进入时的相位：倒放的从尾巴开始。
fn start_phase(params: &[Value], state: &State) -> f64 {
    if speed(params, state) < 0.0 { 1.0 } else { 0.0 }
}

/// 状态里每个片段的权重（权重是 0 的也在）。
fn children(params: &[Value], state: &State) -> Vec<(usize, f32)> {
    match &state.motion {
        Motion::Clip(c) => vec![(*c, 1.0)],
        Motion::Blend { by, clips, at } => {
            let w = if by.len() == 1 {
                blend1d(&at.iter().map(|p| p[0]).collect::<Vec<_>>(), float(params, by[0]))
            } else {
                blend2d(at, [float(params, by[0]), float(params, by[1])])
            };
            clips.iter().copied().zip(w).collect()
        }
    }
}

/// 有效时长：时长已知的片段按权重平均（相位同步）。
fn effective(children: &[(usize, f32)], durations: &[f32]) -> f32 {
    let (sum, weight) = children.iter().filter(|(c, w)| durations[*c] > 0.0 && *w > 0.0).fold((0.0, 0.0), |(s, t), (c, w)| (s + durations[*c] * w, t + w));
    if weight > 0.0 { sum / weight } else { 0.0 }
}

/// 发事件的是 (实例, 片段)：按 实例权重 × 片段权重 取最重的，一样重时后面的赢（后进入的实例、混合树里写在后面的片段），和 2D 选帧同一个规则。
fn loudest(instances: &[Instance], kids: &[Vec<(usize, f32)>]) -> Option<(usize, usize)> {
    let mut best: Option<((usize, usize), f32)> = None;
    for (k, inst) in instances.iter().enumerate() {
        for &(c, w) in &kids[k] {
            let w = inst.weight * w;
            if best.is_none_or(|(_, b)| w >= b) {
                best = Some(((k, c), w));
            }
        }
    }
    best.map(|(kc, _)| kc)
}

/// 片段 `c` 在相位 `p0` → `p1` 之间（`fresh` 时含起点）的事件。
fn clip_events(graph: &Graph, layer: &Layer, state: &State, c: usize, durations: &[f32], (p0, p1): (f64, f64), fresh: bool, fired: &mut Vec<Fired>) {
    let clip = &graph.clips[c];
    let d = durations[c];
    let times: Vec<f32> = clip.events.iter().map(|e| e.0).collect();
    for i in crossed(&times, d, state.looping, p0 * f64::from(d), p1 * f64::from(d), fresh) {
        fired.push(Fired { layer: layer.name.clone(), state: state.name.clone(), clip: clip.name.clone(), name: clip.events[i].1.clone() });
    }
}

/// 新进入的实例在进入的那一步发起点的事件（不管它这时多重），以后按最重的规则。
fn entry_events(graph: &Graph, layer: &Layer, inst: &mut Instance, params: &[Value], durations: &[f32], fired: &mut Vec<Fired>) {
    let state = &layer.states[inst.state];
    let kids = children(params, state);
    let one = Instance { weight: 1.0, ..inst.clone() };
    if let Some((_, c)) = loudest(std::slice::from_ref(&one), std::slice::from_ref(&kids)) {
        clip_events(graph, layer, state, c, durations, (inst.phase, inst.phase), true, fired);
    }
    inst.fresh = false;
}

/// 推进每个实例；按 [`loudest`] 选出的那个片段发事件（层权重是 0 时不发）。
#[allow(clippy::too_many_arguments)]
fn advance(graph: &Graph, layer: &Layer, run: &mut LayerRun, params: &[Value], durations: &[f32], dt: f64, events: bool, fired: &mut Vec<Fired>) {
    let kids: Vec<Vec<(usize, f32)>> = run.instances.iter().map(|i| children(params, &layer.states[i.state])).collect();
    let loud = if events { loudest(&run.instances, &kids) } else { None };
    for (k, inst) in run.instances.iter_mut().enumerate() {
        let state = &layer.states[inst.state];
        let dur = f64::from(effective(&kids[k], durations));
        let speed = speed(params, state);
        let p0 = inst.phase;
        let p1 = if dur > 0.0 {
            p0 + dt * speed / dur
        } else if state.looping {
            p0
        } else if speed < 0.0 {
            0.0
        } else {
            1.0
        };
        let p1 = if state.looping { p1 } else { p1.clamp(0.0, 1.0) };
        inst.played += (p1 - p0).abs();
        inst.phase = p1;
        if let Some((lk, c)) = loud
            && lk == k
        {
            clip_events(graph, layer, state, c, durations, (p0, p1), inst.fresh, fired);
        }
        inst.fresh = false;
        // 循环久了相位越来越大：减掉整圈，免得丢精度
        if state.looping && inst.phase.abs() > 1024.0 {
            inst.phase = inst.phase.rem_euclid(1.0);
        }
        inst.played = inst.played.min(1e9);
    }
}

fn holds(cond: &Cond, params: &[Value]) -> bool {
    match *cond {
        Cond::True(p) => matches!(params[p], Value::Bool(true) | Value::Trigger(true)),
        Cond::False(p) => matches!(params[p], Value::Bool(false)),
        Cond::Cmp(p, op, v) => {
            let x = float(params, p);
            match op {
                Op::Gt => x > v,
                Op::Ge => x >= v,
                Op::Lt => x < v,
                Op::Le => x <= v,
                Op::Eq => x == v,
                Op::Ne => x != v,
            }
        }
    }
}

/// 先看任意状态的（跳过转到现在状态的），再看现在状态的，第一条成立的生效。
fn pick<'a>(layer: &'a Layer, run: &LayerRun, params: &[Value]) -> Option<&'a Transition> {
    let inst = &run.instances[run.current];
    let ok = |t: &Transition| t.exit.is_none_or(|e| inst.played + EXIT_SLACK >= f64::from(e)) && t.when.iter().all(|c| holds(c, params));
    layer.any.iter().filter(|t| t.to != inst.state).find(|t| ok(t)).or_else(|| layer.states[inst.state].to.iter().find(|t| ok(t)))
}

fn output(layer: &Layer, run: &LayerRun, params: &[Value], durations: &[f32]) -> Vec<ClipSample> {
    let mut out = Vec::new();
    for inst in run.instances.iter().filter(|i| i.weight > 0.0) {
        let state = &layer.states[inst.state];
        for (c, w) in children(params, state).into_iter().filter(|(_, w)| *w > 0.0) {
            let d = f64::from(durations[c]);
            let p = if state.looping { inst.phase.rem_euclid(1.0) } else { inst.phase.clamp(0.0, 1.0) };
            out.push(ClipSample { clip: c, time: (p * d) as f32, weight: inst.weight * w });
        }
    }
    out
}

/// 一层输出里最重的片段，一样重时后面的赢（和发事件的规则一样，见 docs/anim-graph.md）。2D 按它选帧。
pub fn heaviest(clips: &[ClipSample]) -> Option<&ClipSample> {
    clips.iter().fold(None, |best: Option<&ClipSample>, c| if best.is_none_or(|b| c.weight >= b.weight) { Some(c) } else { best })
}
