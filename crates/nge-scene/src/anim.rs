//! 精灵的帧动画（`sprites[].anim`）和补间（`tween` command），播完发 `animEnd` / `tweenEnd` 事件。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use nge_core::{CommandKey, EventKey};
use nge_ecs::{Outbox, Time};
use nge_physics2d::RigidBody;
use nge_render2d::{Sprite, SpriteImage, Transform2d};
use serde::{Deserialize, Serialize};

use crate::sync::SceneId;

pub const TWEEN: CommandKey<TweenCommand> = CommandKey::new("tween");
pub const ANIM_END: EventKey<AnimEnd> = EventKey::new("animEnd");
pub const TWEEN_END: EventKey<TweenEnd> = EventKey::new("tweenEnd");

/// `sprites[].anim`：按 `fps` 依次播图集里的 `frames`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct AnimNode {
    pub frames: Vec<u32>,
    pub fps: f32,
    #[serde(rename = "loop")]
    pub looped: bool,
}

impl Default for AnimNode {
    fn default() -> Self {
        Self { frames: Vec::new(), fps: 10.0, looped: true }
    }
}

/// 帧动画播完（只有 `loop: false` 的会发）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimEnd {
    pub id: String,
}

/// 补间结束：正常播完，或者属性全被后开始的补间接管（`cancelled`）。一直播的不会正常播完。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TweenEnd {
    pub id: String,
    pub tag: Option<String>,
    pub cancelled: bool,
}

/// 补间要改的属性，不填的不管。`rotation` 在 Rust 里是弧度，TSX 的 `tween` command 里是度（收到时换算）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TweenProps {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub rotation: Option<f32>,
    pub scale: Option<f32>,
    pub alpha: Option<f32>,
}

impl TweenProps {
    fn is_empty(&self) -> bool {
        self.fields().iter().all(Option::is_none)
    }

    fn fields(&self) -> [Option<f32>; 5] {
        [self.x, self.y, self.rotation, self.scale, self.alpha]
    }

    fn from_fields([x, y, rotation, scale, alpha]: [Option<f32>; 5]) -> Self {
        Self { x, y, rotation, scale, alpha }
    }

    /// 去掉 `other` 里有的属性。
    fn without(&self, other: &TweenProps) -> Self {
        let mut fields = self.fields();
        for (f, o) in fields.iter_mut().zip(other.fields()) {
            if o.is_some() {
                *f = None;
            }
        }
        Self::from_fields(fields)
    }

    /// `self` 里有的属性用 `other` 里的值替换（`other` 没有的保持不变）。
    fn overlay(&self, other: &TweenProps) -> Self {
        let mut fields = self.fields();
        for (f, o) in fields.iter_mut().zip(other.fields()) {
            if f.is_some() && o.is_some() {
                *f = o;
            }
        }
        Self::from_fields(fields)
    }

    /// 换算 TSX 的度；dynamic 刚体去掉位置和旋转。
    fn for_command(&self, dynamic: bool) -> Self {
        let props = TweenProps { rotation: self.rotation.map(f32::to_radians), ..*self };
        if dynamic { TweenProps { x: None, y: None, rotation: None, ..props } } else { props }
    }

    /// 实体现在的值（只取 `self` 里有的属性）。
    fn current(&self, t: &Transform2d, sprite: Option<&Sprite>) -> Self {
        let now = [Some(t.translation.x), Some(t.translation.y), Some(t.rotation), Some(t.scale.x), sprite.map(|s| s.color.w)];
        let mut fields = self.fields();
        for (f, n) in fields.iter_mut().zip(now) {
            *f = f.and(n);
        }
        Self::from_fields(fields)
    }

    fn write(&self, t: &mut Transform2d, sprite: Option<&mut Sprite>) {
        if let Some(x) = self.x {
            t.translation.x = x;
        }
        if let Some(y) = self.y {
            t.translation.y = y;
        }
        if let Some(r) = self.rotation {
            t.rotation = r;
        }
        if let Some(s) = self.scale {
            t.scale = glam::Vec2::splat(s);
        }
        if let (Some(a), Some(sprite)) = (self.alpha, sprite) {
            sprite.color.w = a;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ease {
    Linear,
    EaseIn,
    #[default]
    EaseOut,
    EaseInOut,
    /// 冲过头再回来。
    BackOut,
    /// 落地弹跳。
    BounceOut,
    /// 弹簧。
    ElasticOut,
}

impl Ease {
    /// `t` 从 0 到 1，返回 0 到 1（`BackOut` / `ElasticOut` 中间会超出）。
    pub fn apply(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        match self {
            Ease::Linear => t,
            Ease::EaseIn => t * t * t,
            Ease::EaseOut => 1.0 - (1.0 - t).powi(3),
            Ease::EaseInOut if t < 0.5 => 4.0 * t * t * t,
            Ease::EaseInOut => 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0,
            Ease::BackOut => {
                let (c1, c3) = (1.70158, 2.70158);
                1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
            }
            Ease::BounceOut => bounce_out(t),
            Ease::ElasticOut if t == 0.0 || t == 1.0 => t,
            Ease::ElasticOut => 2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * std::f32::consts::TAU / 3.0).sin() + 1.0,
        }
    }
}

fn bounce_out(t: f32) -> f32 {
    let (n, d) = (7.5625, 2.75);
    if t < 1.0 / d {
        n * t * t
    } else if t < 2.0 / d {
        let t = t - 1.5 / d;
        n * t * t + 0.75
    } else if t < 2.5 / d {
        let t = t - 2.25 / d;
        n * t * t + 0.9375
    } else {
        let t = t - 2.625 / d;
        n * t * t + 0.984375
    }
}

/// TSX 的 `tween` command。
#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct TweenCommand {
    pub id: String,
    /// 显式的起点；不填的属性从开始时的当前值取。
    pub from: TweenProps,
    pub to: TweenProps,
    pub duration: f32,
    pub delay: f32,
    pub ease: Ease,
    pub repeat: i32,
    pub yoyo: bool,
    pub tag: Option<String>,
}

impl Default for TweenCommand {
    fn default() -> Self {
        Self { id: String::new(), from: TweenProps::default(), to: TweenProps::default(), duration: 0.3, delay: 0.0, ease: Ease::default(), repeat: 0, yoyo: false, tag: None }
    }
}

/// 挂在有图集 `Sprite` 的实体上，按时间换帧。
#[derive(Component, Debug, Clone, PartialEq)]
pub struct FrameAnimation {
    frames: Vec<u32>,
    fps: f32,
    looped: bool,
    time: f32,
    done: bool,
}

impl FrameAnimation {
    /// `frames` 为空或 `fps` 不是正数时返回 `None`。
    pub fn new(frames: Vec<u32>, fps: f32, looped: bool) -> Option<Self> {
        (!frames.is_empty() && fps.is_finite() && fps > 0.0).then_some(Self { frames, fps, looped, time: 0.0, done: false })
    }

    pub fn frames(&self) -> &[u32] {
        &self.frames
    }

    pub fn fps(&self) -> f32 {
        self.fps
    }

    pub fn looped(&self) -> bool {
        self.looped
    }

    /// 当前画哪一格。
    pub fn frame(&self) -> u32 {
        let n = self.frames.len();
        if n == 0 {
            return 0;
        }
        let i = (self.time * self.fps) as usize;
        self.frames[if self.looped { i % n } else { i.min(n - 1) }]
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    /// 往前走 `dt` 秒；不循环的刚播完时返回 true。
    fn advance(&mut self, dt: f32) -> bool {
        if self.done {
            return false;
        }
        let length = self.frames.len() as f32 / self.fps;
        self.time += dt;
        if self.looped {
            self.time %= length;
            false
        } else {
            self.done = self.time >= length;
            self.done
        }
    }
}

/// 一个补间。开始（`delay` 过后）时从实体当前的值取起点（`from` 里写了的除外），
/// 并接管它写到的属性：已经开始的其他补间不再写这些属性。
#[derive(Debug, Clone, PartialEq)]
pub struct Tween {
    /// 显式的起点，不填的属性从当前值取。
    pub from: TweenProps,
    pub to: TweenProps,
    pub duration: f32,
    pub delay: f32,
    pub ease: Ease,
    /// 额外再播几次，负数是一直播。
    pub repeat: i32,
    /// 来回播：第二次从 `to` 回到起点。
    pub yoyo: bool,
    pub tag: Option<String>,
    start: Option<TweenProps>,
    elapsed: f32,
}

impl Tween {
    pub fn new(to: TweenProps, duration: f32) -> Self {
        Self { from: TweenProps::default(), to, duration, delay: 0.0, ease: Ease::default(), repeat: 0, yoyo: false, tag: None, start: None, elapsed: 0.0 }
    }

    pub fn from(self, from: TweenProps) -> Self {
        Self { from, ..self }
    }

    pub fn ease(self, ease: Ease) -> Self {
        Self { ease, ..self }
    }

    pub fn delay(self, delay: f32) -> Self {
        Self { delay, ..self }
    }

    pub fn repeat(self, repeat: i32, yoyo: bool) -> Self {
        Self { repeat, yoyo, ..self }
    }

    pub fn tag(self, tag: Option<String>) -> Self {
        Self { tag, ..self }
    }

    /// 往前走 `dt` 秒，返回这一刻的进度（0 = 起点，1 = `to`，已经套了 ease）和是否播完；还在 `delay` 里时是 `None`。
    fn advance(&mut self, dt: f32) -> Option<(f32, bool)> {
        self.elapsed += dt;
        let delay = if self.delay.is_finite() { self.delay.max(0.0) } else { 0.0 };
        let local = self.elapsed - delay;
        if local < 0.0 {
            return None;
        }
        if !(self.duration.is_finite() && self.duration > 0.0) {
            return Some((1.0, true));
        }
        let period = if self.yoyo { 2.0 } else { 1.0 } * self.duration;
        if self.repeat < 0 && local >= period {
            // 一直播的把时间绕回来，免得越加越大丢精度
            self.elapsed = delay + local % period;
        }
        let local = self.elapsed - delay;
        let n = (local / self.duration).floor();
        let cycles = self.repeat as f32 + 1.0;
        let (n, p, done) = if self.repeat >= 0 && n >= cycles { (cycles - 1.0, 1.0, true) } else { (n, local / self.duration - n, false) };
        let backward = self.yoyo && n % 2.0 == 1.0;
        Some((if backward { self.ease.apply(1.0 - p) } else { self.ease.apply(p) }, done))
    }

    fn value(&self, progress: f32) -> TweenProps {
        let from = self.start.unwrap_or_default().fields();
        let mut fields = self.to.fields();
        for (f, from) in fields.iter_mut().zip(from) {
            *f = f.zip(from).map(|(to, from)| from + (to - from) * progress);
        }
        TweenProps::from_fields(fields)
    }
}

/// 实体上正在播的补间。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Tweens(pub Vec<Tween>);

impl Tweens {
    /// 加一个补间。它开始（`delay` 过后）时才接管属性。
    pub fn add(&mut self, tween: Tween) {
        self.0.push(tween);
    }

    /// 往前走 `dt` 秒并写到实体上，返回这一帧结束的补间（tag、是不是被接管）。
    fn step(&mut self, dt: f32, transform: &mut Transform2d, mut sprite: Option<&mut Sprite>) -> Vec<(Option<String>, bool)> {
        let list = &mut self.0;
        let progress: Vec<_> = list.iter_mut().map(|t| t.advance(dt)).collect();
        let mut ended = Vec::new();
        // 这一帧开始的补间接管它写到的属性；同一帧开始的，排在后面的（后发的）赢
        for j in 0..list.len() {
            if progress[j].is_none() || list[j].start.is_some() {
                continue;
            }
            list[j].start = Some(list[j].to.current(transform, sprite.as_deref()).overlay(&list[j].from));
            let taken = list[j].to;
            for k in 0..list.len() {
                if k == j || list[k].start.is_none() {
                    continue;
                }
                let before = list[k].to;
                list[k].to = before.without(&taken);
                if !before.is_empty() && list[k].to.is_empty() {
                    ended.push((list[k].tag.clone(), true));
                }
            }
        }
        let mut done = vec![false; list.len()];
        for (k, t) in list.iter().enumerate() {
            if let Some((p, finished)) = progress[k]
                && !t.to.is_empty()
            {
                t.value(p).write(transform, sprite.as_deref_mut());
                if finished {
                    ended.push((t.tag.clone(), false));
                    done[k] = true;
                }
            }
        }
        let mut k = 0;
        list.retain(|t| {
            k += 1;
            !done[k - 1] && !t.to.is_empty()
        });
        ended
    }
}

fn delta(time: &Time) -> f32 {
    if time.delta.is_finite() { time.delta.max(0.0) } else { 0.0 }
}

pub(crate) fn animate(time: Res<Time>, mut sprites: Query<(Option<&SceneId>, &mut FrameAnimation, &mut Sprite)>, mut outbox: ResMut<Outbox>) {
    let dt = delta(&time);
    for (id, mut anim, mut sprite) in &mut sprites {
        if anim.advance(dt)
            && let Some(id) = id
        {
            outbox.send(ANIM_END, &AnimEnd { id: id.0.clone() });
        }
        if let SpriteImage::Atlas(atlas, frame) = sprite.image
            && frame != anim.frame()
        {
            sprite.image = SpriteImage::Atlas(atlas, anim.frame());
        }
    }
}

pub(crate) fn start_tweens(
    mut commands: Commands,
    mut reader: MessageReader<TweenCommand>,
    mut sprites: Query<(Entity, &SceneId, Option<&RigidBody>, Option<&mut Tweens>), With<Sprite>>,
) {
    if reader.is_empty() {
        return;
    }
    let index: HashMap<String, Entity> = sprites.iter().map(|(e, id, ..)| (id.0.clone(), e)).collect();
    // 同一帧给还没有 Tweens 的实体发了几个补间时先在这里合并
    let mut added: HashMap<Entity, Tweens> = HashMap::new();
    for c in reader.read() {
        let Some((entity, _, body, tweens)) = index.get(&c.id).and_then(|&e| sprites.get_mut(e).ok()) else {
            log::warn!("tween：没有 id 为 {:?} 的精灵", c.id);
            continue;
        };
        let dynamic = body == Some(&RigidBody::Dynamic);
        if dynamic && (c.to.x, c.to.y, c.to.rotation) != (None, None, None) {
            log::warn!("tween：{:?} 是 dynamic 刚体，位置和旋转归物理管，不补间", c.id);
        }
        let to = c.to.for_command(dynamic);
        if to.is_empty() {
            log::warn!("tween：{:?} 的 to 里没有能补间的属性", c.id);
            continue;
        }
        let tween = Tween::new(to, c.duration)
            .from(c.from.for_command(dynamic))
            .ease(c.ease)
            .delay(c.delay)
            .repeat(c.repeat, c.yoyo)
            .tag(c.tag.clone());
        match tweens {
            Some(mut t) => t.add(tween),
            None => added.entry(entity).or_default().add(tween),
        }
    }
    for (entity, tweens) in added {
        commands.entity(entity).insert(tweens);
    }
}

pub(crate) fn tween(
    time: Res<Time>,
    mut sprites: Query<(Option<&SceneId>, &mut Tweens, &mut Transform2d, Option<&mut Sprite>)>,
    mut outbox: ResMut<Outbox>,
) {
    let dt = delta(&time);
    for (id, mut tweens, mut transform, sprite) in &mut sprites {
        if tweens.0.is_empty() {
            continue;
        }
        let ended = tweens.step(dt, &mut transform, sprite.map(Mut::into_inner));
        if let Some(id) = id {
            for (tag, cancelled) in ended {
                outbox.send(TWEEN_END, &TweenEnd { id: id.0.clone(), tag, cancelled });
            }
        }
    }
}

#[cfg(test)]
mod tests;
