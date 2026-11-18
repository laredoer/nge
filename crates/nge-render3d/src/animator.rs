//! 播动画：推进时间、淡入淡出、按时间算姿势和变形权重（见 docs/3d-more.md、docs/render3d-more.md）。

use bevy_ecs::prelude::*;
use nge_ecs::Time;

use crate::animation::{Animation, Property, Trs, rotation};
use crate::assets::{Assets3d, Model};
use crate::mesh::MAX_MORPH_TARGETS;
use crate::renderer::Model3d;

/// 播模型里的哪段动画。`time` 按 `speed`（1 是原速）往前走，`looping` 时绕回开头，不然停在最后一帧。
/// 找不到这个名字时停在不播动画的姿势，warn 一次。换动画用 [`Animator::play`]，可以淡入淡出。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Pose)]
pub struct Animator {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub time: f32,
    /// 正在淡出的旧动画；淡完后清掉。
    pub fade: Option<Fade>,
}

/// 淡出中的旧动画：它自己的时间、速度、循环照常走，`elapsed / duration` 是新动画占的比例。
#[derive(Debug, Clone, PartialEq)]
pub struct Fade {
    pub clip: String,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
    pub elapsed: f32,
    pub duration: f32,
}

impl Animator {
    pub fn new(clip: impl Into<String>) -> Self {
        Self { clip: clip.into(), speed: 1.0, looping: true, time: 0.0, fade: None }
    }

    /// 换成 `clip`，`fade` 秒里从现在的动画过渡过去；`fade` 不是有限的正数时硬切。同名时什么也不做。
    ///
    /// 淡入淡出途中再换：
    /// - 换回正在淡出的旧动画：两边对调，旧动画接着自己的时间、速度、循环走，已混合的比例反过来按新的 `fade` 换算，画面不跳；
    /// - 换成第三个：权重大的那一边当旧动画淡出，另一边直接丢掉。
    pub fn play(&mut self, clip: impl Into<String>, fade: f32) {
        let clip = clip.into();
        if clip == self.clip {
            return;
        }
        let fade = (fade.is_finite() && fade > 0.0).then_some(fade);
        let current = Fade { clip: std::mem::replace(&mut self.clip, clip), time: self.time, speed: self.speed, looping: self.looping, elapsed: 0.0, duration: 0.0 };
        let old = match self.fade.take() {
            // 对调：新动画占的比例变成 1 - 原来的
            Some(f) if f.clip == self.clip => {
                (self.time, self.speed, self.looping) = (f.time, f.speed, f.looping);
                self.fade = fade.map(|duration| Fade { elapsed: (1.0 - f.progress()) * duration, duration, ..current });
                return;
            }
            // 第三个：原来的新动画占不到一半时，还在淡出的那个更重，从它淡出
            Some(f) if f.progress() < 0.5 => f,
            _ => current,
        };
        self.fade = fade.map(|duration| Fade { elapsed: 0.0, duration, ..old });
        self.time = 0.0;
    }
}

impl Fade {
    /// 新动画占的比例，0–1。
    fn progress(&self) -> f32 {
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }
}

/// 这一帧的姿势：每个节点相对模型原点的矩阵和变形权重。由动画系统写，渲染器读（只在还有 [`Animator`] 时）；没有它的模型按不播动画的样子画。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Pose {
    pub nodes: Vec<glam::Mat4>,
    /// 每个节点的变形权重：从挂在节点上的部件的默认权重开始，再按动画改。
    pub weights: Vec<[f32; MAX_MORPH_TARGETS]>,
    /// 提示过找不到的动画名，免得每帧报。
    missing: Option<String>,
    locals: Vec<Trs>,
    /// 淡出中的旧动画采样到这里，再和新的混。
    old_locals: Vec<Trs>,
    old_weights: Vec<[f32; MAX_MORPH_TARGETS]>,
}

/// 推进动画、算姿势（`PostUpdate`）。时间按 [`Time`] 的（缩放过的）帧间隔走。有状态机（`AnimGraph`）的实体归 `graph_pose` 管。
pub(crate) fn animate(time: Res<Time>, assets: Res<Assets3d>, mut models: Query<(&Model3d, &mut Animator, &mut Pose), Without<nge_anim::AnimGraph>>) {
    for (model, mut animator, mut pose) in &mut models {
        let Some(model) = assets.model(model.0) else { continue };
        let pose = &mut *pose;
        reset(model, &mut pose.locals, &mut pose.weights);
        match model.animations.iter().find(|a| a.name == animator.clip) {
            Some(clip) => {
                pose.missing = None;
                let t = advance(clip, animator.time, animator.speed, animator.looping, time.delta);
                if animator.time != t {
                    animator.time = t;
                }
                apply(clip, t, &mut pose.locals, &mut pose.weights);
            }
            None if pose.missing.as_deref() != Some(&animator.clip) => {
                let names: Vec<&str> = model.animations.iter().map(|a| a.name.as_str()).collect();
                log::warn!("模型里没有叫 {:?} 的动画（有 {names:?}），停在不播动画的姿势", animator.clip);
                pose.missing = Some(animator.clip.clone());
            }
            None => {}
        }
        if animator.fade.is_some() {
            fade_out(model, &mut animator, pose, time.delta);
        }
        let Pose { nodes, locals, .. } = pose;
        model.skeleton.globals(locals, nodes);
    }
}

/// 旧动画往前走一步，按 `elapsed / duration` 和新动画的姿势混；到时间或旧名字找不到时丢掉旧的。
/// 只在真有变化时才拿可变引用，暂停时不把 [`Animator`] 标成改过。
fn fade_out(model: &Model, animator: &mut Mut<Animator>, pose: &mut Pose, dt: f32) {
    let Some(fade) = animator.fade.as_ref() else { return };
    let elapsed = fade.elapsed + dt;
    let old = model.animations.iter().find(|a| a.name == fade.clip);
    // 时长是 NaN 时比较不成立，也算到时间
    let (Some(old), true) = (old, elapsed < fade.duration) else {
        animator.fade = None;
        return;
    };
    let time = advance(old, fade.time, fade.speed, fade.looping, dt);
    let s = elapsed / fade.duration;
    if (elapsed, time) != (fade.elapsed, fade.time) {
        let fade = animator.fade.as_mut().expect("上面刚读过");
        (fade.elapsed, fade.time) = (elapsed, time);
    }
    reset(model, &mut pose.old_locals, &mut pose.old_weights);
    apply(old, time, &mut pose.old_locals, &mut pose.old_weights);
    for (new, old) in pose.locals.iter_mut().zip(&pose.old_locals) {
        *new = Trs { translation: old.translation.lerp(new.translation, s), rotation: old.rotation.slerp(new.rotation, s), scale: old.scale.lerp(new.scale, s) };
    }
    for (new, old) in pose.weights.iter_mut().zip(&pose.old_weights) {
        for (n, o) in new.iter_mut().zip(old) {
            *n = o + (*n - o) * s;
        }
    }
}

/// 不播动画的本地变换；权重从挂在节点上的部件的默认权重开始。
pub(crate) fn reset(model: &Model, locals: &mut Vec<Trs>, weights: &mut Vec<[f32; MAX_MORPH_TARGETS]>) {
    let nodes = &model.skeleton.nodes;
    locals.clear();
    locals.extend(nodes.iter().map(|n| n.rest));
    weights.clear();
    weights.resize(nodes.len(), [0.0; MAX_MORPH_TARGETS]);
    for part in &model.parts {
        if let Some(w) = part.node.and_then(|n| weights.get_mut(n as usize)) {
            *w = padded(&part.weights);
        }
    }
}

/// 补零或截断到 [`MAX_MORPH_TARGETS`] 个。
pub(crate) fn padded(weights: &[f32]) -> [f32; MAX_MORPH_TARGETS] {
    let mut out = [0.0; MAX_MORPH_TARGETS];
    for (o, w) in out.iter_mut().zip(weights) {
        *o = *w;
    }
    out
}

/// 往前走 `dt` 秒后的时间：循环的绕回开头，不循环的停在两头。
fn advance(clip: &Animation, time: f32, speed: f32, looping: bool, dt: f32) -> f32 {
    let speed = if speed.is_finite() { speed } else { 0.0 };
    let t = time + dt * speed;
    if clip.duration <= 0.0 {
        0.0
    } else if looping {
        t.rem_euclid(clip.duration)
    } else {
        t.clamp(0.0, clip.duration)
    }
}

/// 按时间 `t` 采样每条通道，写进本地变换和权重。
pub(crate) fn apply(clip: &Animation, t: f32, locals: &mut [Trs], weights: &mut [[f32; MAX_MORPH_TARGETS]]) {
    for channel in &clip.channels {
        let n = channel.node as usize;
        let v = channel.sample(t);
        match channel.property {
            Property::Weights(segment) => {
                let start = usize::from(segment) * 4;
                if let Some(w) = weights.get_mut(n).and_then(|w| w.get_mut(start..start + 4)) {
                    w.copy_from_slice(&v.to_array());
                }
            }
            property => {
                let Some(local) = locals.get_mut(n) else { continue };
                match property {
                    Property::Translation => local.translation = v.truncate(),
                    Property::Rotation => local.rotation = rotation(v),
                    Property::Scale => local.scale = v.truncate(),
                    Property::Weights(_) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Quat, Vec3, Vec4};

    use super::*;
    use crate::animation::{Channel, Interpolation, Node, Skeleton};

    fn channel(property: Property, interpolation: Interpolation, times: Vec<f32>, values: Vec<Vec4>) -> Channel {
        Channel { node: 0, property, interpolation, times, values }
    }

    /// 一直停在 `value` 的通道。
    fn hold(property: Property, value: Vec4) -> Channel {
        channel(property, Interpolation::Step, vec![0.0], vec![value])
    }

    fn clip(name: &str, channels: Vec<Channel>) -> Animation {
        Animation { name: name.into(), duration: 1.0, channels }
    }

    /// 一个节点，挂一个默认权重是 `weights` 的部件；动画 slide 在 1 秒里把它从 x = 0 线性移到 x = 10，另加 `extra`。
    fn model_app(animator: Animator, extra: Vec<Animation>, weights: Vec<f32>) -> (nge_ecs::App, Entity) {
        use crate::{MaterialId, Mesh, Part, Render3dPlugin, Transform3d};

        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let slide = channel(Property::Translation, Interpolation::Linear, vec![0.0, 1.0], vec![Vec4::ZERO, Vec4::new(10.0, 0.0, 0.0, 0.0)]);
        let mut assets = app.world_mut().resource_mut::<Assets3d>();
        let mesh = assets.add_mesh(Mesh::cube(1.0)).unwrap();
        let model = Model {
            parts: vec![Part { node: Some(0), weights, ..Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY) }],
            skeleton: Skeleton { nodes: vec![Node { parent: None, rest: Trs { translation: Vec3::Y, ..Default::default() } }], skins: vec![], names: vec![] },
            animations: [vec![clip("slide", vec![slide])], extra].concat(),
        };
        let model = assets.add_model(model).unwrap();
        let entity = app.world_mut().spawn((Model3d(model), Transform3d::default(), animator)).id();
        (app, entity)
    }

    fn sliding_app(animator: Animator) -> (nge_ecs::App, Entity) {
        model_app(animator, vec![], vec![])
    }

    fn tick(app: &mut nge_ecs::App, dt: f32) {
        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, dt);
    }

    /// 推进 `dt` 秒，返回节点的 x 和动画时间。
    fn step(app: &mut nge_ecs::App, entity: Entity, dt: f32) -> (f32, f32) {
        tick(app, dt);
        let world = app.world();
        (world.get::<Pose>(entity).unwrap().nodes[0].w_axis.x, world.get::<Animator>(entity).unwrap().time)
    }

    fn animator(app: &nge_ecs::App, entity: Entity) -> Animator {
        app.world().get::<Animator>(entity).unwrap().clone()
    }

    fn play(app: &mut nge_ecs::App, entity: Entity, clip: &str, fade: f32) {
        app.world_mut().get_mut::<Animator>(entity).unwrap().play(clip, fade);
    }

    #[test]
    fn animator_advances_loops_and_clamps() {
        let (mut app, e) = sliding_app(Animator::new("slide"));
        assert_eq!(step(&mut app, e, 0.25), (2.5, 0.25));
        assert_eq!(step(&mut app, e, 1.0), (2.5, 0.25), "循环：1.25 绕回 0.25");

        let (mut app, e) = sliding_app(Animator { speed: 2.0, looping: false, ..Animator::new("slide") });
        assert_eq!(step(&mut app, e, 0.25), (5.0, 0.5), "两倍速");
        assert_eq!(step(&mut app, e, 1.0), (10.0, 1.0), "不循环：停在最后一帧");

        let (mut app, e) = sliding_app(Animator { speed: -1.0, ..Animator::new("slide") });
        assert_eq!(step(&mut app, e, 0.25), (7.5, 0.75), "倒着播");
        let (mut app, e) = sliding_app(Animator { speed: -1.0, looping: false, ..Animator::new("slide") });
        assert_eq!(step(&mut app, e, 0.25), (0.0, 0.0), "倒着播不循环：停在开头");
    }

    #[test]
    fn unknown_clip_holds_the_rest_pose() {
        let (mut app, e) = sliding_app(Animator::new("nope"));
        let (x, _) = step(&mut app, e, 0.25);
        assert_eq!(x, 0.0);
        assert_eq!(app.world().get::<Pose>(e).unwrap().nodes[0].w_axis.y, 1.0, "不播动画的姿势");
        // 改成存在的名字后接着播
        app.world_mut().get_mut::<Animator>(e).unwrap().clip = "slide".into();
        assert_eq!(step(&mut app, e, 0.5).0, 5.0);
    }

    #[test]
    fn play_keeps_the_old_clip_for_fading() {
        let mut a = Animator { speed: 2.0, looping: false, time: 0.3, ..Animator::new("walk") };
        a.play("run", 0.5);
        assert_eq!((a.clip.as_str(), a.time), ("run", 0.0), "新动画从头开始");
        assert_eq!(a.fade, Some(Fade { clip: "walk".into(), time: 0.3, speed: 2.0, looping: false, elapsed: 0.0, duration: 0.5 }));
    }

    #[test]
    fn play_same_clip_does_nothing() {
        let mut a = Animator { time: 0.3, ..Animator::new("walk") };
        a.play("walk", 0.5);
        assert_eq!(a, Animator { time: 0.3, ..Animator::new("walk") }, "同名不重来");
    }

    #[test]
    fn play_without_fade_cuts() {
        for fade in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let mut a = Animator { time: 0.3, ..Animator::new("walk") };
            a.play("jump", 0.5);
            a.play("run", fade);
            assert_eq!((a.clip.as_str(), a.time, a.fade.clone()), ("run", 0.0, None), "fade = {fade}：硬切，清掉淡出");
        }
    }

    /// slide 停在 x = 0 时换到 x = 10、转 90° 的 there；旧的 slide 在淡出时照常往前走。
    #[test]
    fn fading_blends_old_and_new_then_drops_the_old() {
        let quarter = Vec4::from(Quat::from_rotation_z(FRAC_PI_2));
        let there = clip("there", vec![hold(Property::Translation, Vec4::new(10.0, 0.0, 0.0, 0.0)), hold(Property::Rotation, quarter)]);
        let (mut app, e) = model_app(Animator { speed: 0.0, ..Animator::new("slide") }, vec![there], vec![]);
        step(&mut app, e, 0.1);
        app.world_mut().get_mut::<Animator>(e).unwrap().speed = 1.0;
        play(&mut app, e, "there", 1.0);
        // 旧的 slide 走到 0.5（x = 5），新的在 10：一半一半是 7.5
        let (x, t) = step(&mut app, e, 0.5);
        assert!((x - 7.5).abs() < 1e-4, "{x}");
        assert_eq!(t, 0.5);
        let fade = animator(&app, e).fade.unwrap();
        assert_eq!((fade.time, fade.elapsed), (0.5, 0.5), "旧动画照常往前走");
        let (_, rotation, _) = app.world().get::<Pose>(e).unwrap().nodes[0].to_scale_rotation_translation();
        assert!(rotation.angle_between(Quat::from_rotation_z(FRAC_PI_2 / 2.0)) < 1e-4, "旋转 slerp 到一半：{rotation}");

        let (x, _) = step(&mut app, e, 0.6);
        assert_eq!(x, 10.0, "淡完了只剩新动画");
        assert_eq!(animator(&app, e).fade, None, "丢掉旧的");
    }

    #[test]
    fn unknown_old_clip_shows_the_new_one_directly() {
        let there = clip("there", vec![hold(Property::Translation, Vec4::new(10.0, 0.0, 0.0, 0.0))]);
        let (mut app, e) = model_app(Animator::new("nope"), vec![there], vec![]);
        play(&mut app, e, "there", 1.0);
        assert_eq!(step(&mut app, e, 0.25).0, 10.0);
    }

    fn weights(app: &nge_ecs::App, entity: Entity) -> [f32; MAX_MORPH_TARGETS] {
        app.world().get::<Pose>(entity).unwrap().weights[0]
    }

    #[test]
    fn weights_start_from_the_part_and_follow_channels() {
        let (mut app, e) = model_app(Animator::new("slide"), vec![], vec![0.2, 0.4]);
        tick(&mut app, 0.1);
        assert_eq!(weights(&app, e), [0.2, 0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], "没有权重通道时用部件的默认权重");

        let second = clip("second", vec![hold(Property::Weights(1), Vec4::new(1.0, 2.0, 3.0, 4.0))]);
        let (mut app, e) = model_app(Animator::new("second"), vec![second], vec![0.2, 0.4]);
        tick(&mut app, 0.1);
        assert_eq!(weights(&app, e), [0.2, 0.4, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0], "第 1 段是第 4–7 个权重");
    }

    #[test]
    fn fading_blends_weights() {
        let squish = clip("squish", vec![hold(Property::Weights(0), Vec4::new(1.0, 0.5, 0.0, 0.0))]);
        let (mut app, e) = model_app(Animator::new("slide"), vec![squish], vec![0.2]);
        tick(&mut app, 0.1);
        play(&mut app, e, "squish", 1.0);
        tick(&mut app, 0.5);
        let w = weights(&app, e);
        assert!((w[0] - 0.6).abs() < 1e-5 && (w[1] - 0.25).abs() < 1e-5, "0.2 → 1、0 → 0.5 的一半：{w:?}");
    }
    /// slide（x 从 0 走到 10）播到 0.2 时淡到 there（停在 x = 10），混到一半再 play slide：反向淡回去。
    #[test]
    fn playing_the_fading_clip_again_reverses_without_a_jump() {
        let there = clip("there", vec![hold(Property::Translation, Vec4::new(10.0, 0.0, 0.0, 0.0))]);
        let (mut app, e) = model_app(Animator::new("slide"), vec![there], vec![]);
        step(&mut app, e, 0.2);
        play(&mut app, e, "there", 1.0);
        let (before, _) = step(&mut app, e, 0.5);
        assert!((before - 8.5).abs() < 1e-4, "slide 在 0.7（x = 7）、there 在 10，各一半：{before}");
        play(&mut app, e, "slide", 1.0);
        let a = animator(&app, e);
        assert_eq!((a.clip.as_str(), a.time), ("slide", 0.7), "恢复 slide 的时间，不从头来");
        let fade = a.fade.unwrap();
        assert_eq!(fade.clip, "there");
        assert!((fade.elapsed - 0.5).abs() < 1e-5, "新进度 = 1 - 旧进度：{}", fade.elapsed);
        let (after, t) = step(&mut app, e, 0.01);
        assert!((after - before).abs() < 0.05, "画面不跳：{before} → {after}");
        assert!((t - 0.71).abs() < 1e-5, "slide 的时间接着走：{t}");
    }

    #[test]
    fn reversing_scales_the_progress_to_the_new_fade_length() {
        let mut a = Animator { speed: 2.0, looping: false, time: 0.3, ..Animator::new("idle") };
        a.play("squish", 1.0);
        a.time = 0.4;
        a.fade.as_mut().unwrap().elapsed = 0.25;
        a.fade.as_mut().unwrap().time = 0.55;
        a.speed = 1.0;
        a.play("idle", 2.0);
        assert_eq!((a.clip.as_str(), a.time, a.speed, a.looping), ("idle", 0.55, 2.0, false), "恢复 idle 自己的时间、速度、循环");
        assert_eq!(a.fade, Some(Fade { clip: "squish".into(), time: 0.4, speed: 1.0, looping: false, elapsed: 1.5, duration: 2.0 }), "squish 占 0.25，反过来 idle 占 0.75");
    }

    #[test]
    fn a_third_clip_fades_from_the_heavier_side() {
        let start = || {
            let mut a = Animator { time: 0.3, ..Animator::new("idle") };
            a.play("squish", 1.0);
            a.time = 0.1;
            a
        };
        // squish 只占 0.3：idle 更重，从 idle 淡出
        let mut a = start();
        a.fade.as_mut().unwrap().elapsed = 0.3;
        a.play("walk", 0.5);
        let fade = a.fade.unwrap();
        assert_eq!((a.clip.as_str(), a.time, fade.clip.as_str(), fade.time, fade.elapsed), ("walk", 0.0, "idle", 0.3, 0.0));
        // squish 占 0.7：从 squish 淡出
        let mut a = start();
        a.fade.as_mut().unwrap().elapsed = 0.7;
        a.play("walk", 0.5);
        let fade = a.fade.unwrap();
        assert_eq!((a.clip.as_str(), fade.clip.as_str(), fade.time, fade.elapsed), ("walk", "squish", 0.1, 0.0));
    }

    #[derive(Resource, Default)]
    struct Changes(usize);

    /// 在动画系统之后数这一帧被改过的 Animator。
    fn changes_after_animate(app: &mut nge_ecs::App) {
        app.init_resource::<Changes>();
        app.add_systems(nge_ecs::PostUpdate, (|q: Query<(), Changed<Animator>>, mut n: ResMut<Changes>| n.0 = q.iter().count()).after(animate));
    }

    #[test]
    fn animator_is_not_marked_changed_when_nothing_moves() {
        let (mut app, e) = sliding_app(Animator { speed: 0.0, ..Animator::new("slide") });
        changes_after_animate(&mut app);
        tick(&mut app, 0.1);
        tick(&mut app, 0.1);
        assert_eq!(app.world().resource::<Changes>().0, 0, "没有淡出、时间不变");

        // 淡出中但暂停了（帧间隔 0）
        play(&mut app, e, "slide2", 1.0);
        app.world_mut().get_mut::<Animator>(e).unwrap().fade.as_mut().unwrap().clip = "slide".into();
        tick(&mut app, 0.0);
        tick(&mut app, 0.0);
        assert_eq!(app.world().resource::<Changes>().0, 0, "暂停时淡出也不动");
        tick(&mut app, 0.1);
        assert_eq!(app.world().resource::<Changes>().0, 1, "真在淡出时照常标记");
    }
}
