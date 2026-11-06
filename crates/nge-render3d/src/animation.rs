//! 模型的节点树、蒙皮和动画（glTF 的 skins / animations），以及按时间算姿势（见 docs/3d-more.md）。

use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec3, Vec4};
use nge_ecs::Time;

use crate::assets::Assets3d;
use crate::renderer::Model3d;

/// 平移、旋转、缩放。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Trs {
    fn default() -> Self {
        Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Trs {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

/// 节点树里的一个节点。`parent` 总在它前面（按父在前的顺序排）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub parent: Option<u32>,
    /// 不播动画时的本地变换。
    pub rest: Trs,
}

/// 一套蒙皮：关节（节点下标）和每个关节的逆绑定矩阵。顶点里的关节编号是 `joints` 里的下标。
#[derive(Debug, Clone, PartialEq)]
pub struct Skin {
    pub joints: Vec<u32>,
    pub inverse_bind: Vec<Mat4>,
}

/// 模型的节点树和蒙皮。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub nodes: Vec<Node>,
    pub skins: Vec<Skin>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    Translation,
    Rotation,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    /// 每个关键帧三个值：入切线、值、出切线。
    CubicSpline,
}

/// 一条动画通道：某个节点的某个属性随时间变化。`values` 里平移、缩放用 xyz，旋转是四元数 xyzw。
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub node: u32,
    pub property: Property,
    pub interpolation: Interpolation,
    /// 递增的时间（秒）。
    pub times: Vec<f32>,
    pub values: Vec<Vec4>,
}

/// 一段动画（glTF 的一个 animation）。
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub name: String,
    /// 最后一个关键帧的时间。
    pub duration: f32,
    pub channels: Vec<Channel>,
}

/// 四元数（xyzw）归一化；零或不是有限数时当作不转。
pub(crate) fn rotation(v: Vec4) -> Quat {
    let q = Quat::from_vec4(v);
    if q.is_finite() && q.length_squared() > 0.0 { q.normalize() } else { Quat::IDENTITY }
}

impl Channel {
    /// 这条通道哪里不对：目标节点不存在、关键帧数和值的个数对不上、时间不是不递减的有限数。
    pub(crate) fn problem(&self, nodes: usize) -> Option<String> {
        let per_key = if self.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
        if self.node as usize >= nodes {
            Some("目标节点不存在".to_owned())
        } else if self.times.is_empty() || self.values.len() != self.times.len() * per_key {
            Some(format!("{} 个关键帧配了 {} 个值", self.times.len(), self.values.len()))
        } else if self.times.iter().any(|t| !t.is_finite()) || self.times.windows(2).any(|w| w[1] < w[0]) {
            Some("时间要是不递减的有限数".to_owned())
        } else {
            None
        }
    }

    /// 在时间 `t` 的值：第一帧之前取第一帧，最后一帧之后取最后一帧。
    pub fn sample(&self, t: f32) -> Vec4 {
        let cubic = self.interpolation == Interpolation::CubicSpline;
        let value = |k: usize| if cubic { self.values[k * 3 + 1] } else { self.values[k] };
        let last = self.times.len() - 1;
        // 第一个时间大于 t 的关键帧
        let next = self.times.partition_point(|&k| k <= t);
        if next == 0 {
            return value(0);
        }
        if next > last {
            return value(last);
        }
        let (k, t0, t1) = (next - 1, self.times[next - 1], self.times[next]);
        let dt = t1 - t0;
        let s = if dt > 0.0 { (t - t0) / dt } else { 0.0 };
        let rotation = self.property == Property::Rotation;
        match self.interpolation {
            Interpolation::Step => value(k),
            Interpolation::Linear if rotation => Vec4::from(Quat::from_vec4(value(k)).slerp(Quat::from_vec4(value(k + 1)), s)),
            Interpolation::Linear => value(k).lerp(value(k + 1), s),
            Interpolation::CubicSpline => {
                // Hermite：p0、出切线 m0、p1、入切线 m1（切线按帧间隔缩放）
                let (p0, m0, m1, p1) = (value(k), self.values[k * 3 + 2] * dt, self.values[(k + 1) * 3] * dt, value(k + 1));
                let (s2, s3) = (s * s, s * s * s);
                let v = p0 * (2.0 * s3 - 3.0 * s2 + 1.0) + m0 * (s3 - 2.0 * s2 + s) + p1 * (-2.0 * s3 + 3.0 * s2) + m1 * (s3 - s2);
                if rotation { v.normalize_or_zero() } else { v }
            }
        }
    }
}

impl Skeleton {
    /// 父节点在前、蒙皮和动画引用的节点存在、关键帧数对得上。
    pub(crate) fn validate(&self, animations: &[Animation]) -> Result<(), String> {
        for (i, node) in self.nodes.iter().enumerate() {
            if node.parent.is_some_and(|p| p as usize >= i) {
                return Err(format!("节点 {i} 的父节点 {:?} 要排在它前面", node.parent));
            }
        }
        for (i, skin) in self.skins.iter().enumerate() {
            if skin.joints.len() != skin.inverse_bind.len() {
                return Err(format!("蒙皮 {i} 有 {} 个关节、{} 个逆绑定矩阵", skin.joints.len(), skin.inverse_bind.len()));
            }
            if let Some(j) = skin.joints.iter().find(|&&j| j as usize >= self.nodes.len()) {
                return Err(format!("蒙皮 {i} 的关节 {j} 不是节点"));
            }
        }
        for animation in animations {
            for channel in &animation.channels {
                if let Some(bad) = channel.problem(self.nodes.len()) {
                    return Err(format!("动画 {:?} 的通道（节点 {}）：{bad}", animation.name, channel.node));
                }
            }
        }
        Ok(())
    }

    /// 本地变换 → 每个节点相对模型原点的矩阵。
    pub fn globals(&self, locals: &[Trs], out: &mut Vec<Mat4>) {
        out.clear();
        for (node, local) in self.nodes.iter().zip(locals) {
            let m = local.matrix();
            out.push(match node.parent {
                Some(p) => out[p as usize] * m,
                None => m,
            });
        }
    }

    /// 不播动画时每个节点的矩阵。
    pub fn rest(&self) -> Vec<Mat4> {
        let locals: Vec<Trs> = self.nodes.iter().map(|n| n.rest).collect();
        let mut out = Vec::new();
        self.globals(&locals, &mut out);
        out
    }

    /// 第 `skin` 套蒙皮的关节矩阵（节点矩阵 × 逆绑定矩阵），追加到 `out`。
    pub fn joints(&self, skin: usize, globals: &[Mat4], out: &mut Vec<Mat4>) {
        let Some(skin) = self.skins.get(skin) else { return };
        out.extend(skin.joints.iter().zip(&skin.inverse_bind).map(|(&j, ib)| globals[j as usize] * *ib));
    }
}

/// 播模型里的哪段动画。`time` 按 `speed`（1 是原速）往前走，`looping` 时绕回开头，不然停在最后一帧。
/// 找不到这个名字时停在不播动画的姿势，warn 一次。
#[derive(Component, Debug, Clone, PartialEq)]
#[require(Pose)]
pub struct Animator {
    pub clip: String,
    pub speed: f32,
    pub looping: bool,
    pub time: f32,
}

impl Animator {
    pub fn new(clip: impl Into<String>) -> Self {
        Self { clip: clip.into(), speed: 1.0, looping: true, time: 0.0 }
    }
}

/// 这一帧的姿势：每个节点相对模型原点的矩阵。由动画系统写，渲染器读（只在还有 [`Animator`] 时）；没有它的模型按不播动画的样子画。
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Pose {
    pub nodes: Vec<Mat4>,
    /// 提示过找不到的动画名，免得每帧报。
    missing: Option<String>,
    locals: Vec<Trs>,
}

/// 推进动画、算姿势（`PostUpdate`）。时间按 [`Time`] 的（缩放过的）帧间隔走。
pub(crate) fn animate(time: Res<Time>, assets: Res<Assets3d>, mut models: Query<(&Model3d, &mut Animator, &mut Pose)>) {
    for (model, mut animator, mut pose) in &mut models {
        let Some(model) = assets.model(model.0) else { continue };
        let skeleton = &model.skeleton;
        let pose = &mut *pose;
        pose.locals.clear();
        pose.locals.extend(skeleton.nodes.iter().map(|n| n.rest));
        match model.animations.iter().find(|a| a.name == animator.clip) {
            Some(clip) => {
                pose.missing = None;
                let speed = if animator.speed.is_finite() { animator.speed } else { 0.0 };
                let mut t = animator.time + time.delta * speed;
                t = if clip.duration <= 0.0 {
                    0.0
                } else if animator.looping {
                    t.rem_euclid(clip.duration)
                } else {
                    t.clamp(0.0, clip.duration)
                };
                if animator.time != t {
                    animator.time = t;
                }
                for channel in &clip.channels {
                    let Some(local) = pose.locals.get_mut(channel.node as usize) else { continue };
                    let v = channel.sample(t);
                    match channel.property {
                        Property::Translation => local.translation = v.truncate(),
                        Property::Rotation => local.rotation = rotation(v),
                        Property::Scale => local.scale = v.truncate(),
                    }
                }
            }
            None if pose.missing.as_deref() != Some(&animator.clip) => {
                let names: Vec<&str> = model.animations.iter().map(|a| a.name.as_str()).collect();
                log::warn!("模型里没有叫 {:?} 的动画（有 {names:?}），停在不播动画的姿势", animator.clip);
                pose.missing = Some(animator.clip.clone());
            }
            None => {}
        }
        let Pose { nodes, locals, .. } = pose;
        skeleton.globals(locals, nodes);
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;

    fn channel(property: Property, interpolation: Interpolation, times: Vec<f32>, values: Vec<Vec4>) -> Channel {
        Channel { node: 0, property, interpolation, times, values }
    }

    #[test]
    fn linear_step_and_clamping() {
        let c = channel(Property::Translation, Interpolation::Linear, vec![1.0, 3.0], vec![Vec4::ZERO, Vec4::new(4.0, 0.0, 0.0, 0.0)]);
        assert_eq!(c.sample(0.0).x, 0.0, "第一帧之前取第一帧");
        assert_eq!(c.sample(2.0).x, 2.0);
        assert_eq!(c.sample(9.0).x, 4.0, "最后一帧之后取最后一帧");
        let step = Channel { interpolation: Interpolation::Step, ..c.clone() };
        assert_eq!(step.sample(2.9).x, 0.0);
        assert_eq!(step.sample(3.0).x, 4.0);
    }

    #[test]
    fn broken_quaternions_mean_no_rotation() {
        assert_eq!(rotation(Vec4::ZERO), Quat::IDENTITY);
        assert_eq!(rotation(Vec4::splat(f32::NAN)), Quat::IDENTITY);
        assert!(rotation(Vec4::new(0.0, 0.0, 2.0, 0.0)).abs_diff_eq(Quat::from_xyzw(0.0, 0.0, 1.0, 0.0), 1e-6));
    }

    #[test]
    fn rotations_slerp() {
        let quarter = Vec4::from(Quat::from_rotation_z(FRAC_PI_2));
        let c = channel(Property::Rotation, Interpolation::Linear, vec![0.0, 1.0], vec![Vec4::from(Quat::IDENTITY), quarter]);
        let half = Quat::from_vec4(c.sample(0.5));
        assert!(half.angle_between(Quat::from_rotation_z(FRAC_PI_2 / 2.0)) < 1e-4, "{half}");
    }

    #[test]
    fn cubic_spline_passes_through_keys_and_uses_tangents() {
        // 关键帧 0 → 2，两头切线都是 0：中点是 1（对称），1/4 处比线性的慢
        let v = |x: f32| Vec4::new(x, 0.0, 0.0, 0.0);
        let c = channel(Property::Translation, Interpolation::CubicSpline, vec![0.0, 2.0], vec![v(0.0), v(0.0), v(0.0), v(0.0), v(2.0), v(0.0)]);
        assert_eq!(c.sample(0.0).x, 0.0);
        assert_eq!(c.sample(2.0).x, 2.0);
        assert!((c.sample(1.0).x - 1.0).abs() < 1e-5);
        assert!(c.sample(0.5).x < 0.5);
        // 出切线是 4 时开头冲得快
        let steep = channel(Property::Translation, Interpolation::CubicSpline, vec![0.0, 2.0], vec![v(0.0), v(0.0), v(4.0), v(0.0), v(2.0), v(0.0)]);
        assert!(steep.sample(0.5).x > c.sample(0.5).x);
        // 切线按帧间隔（2）缩放：中点 = 1 + 出切线 4 × 2 × 1/8
        assert!((steep.sample(1.0).x - 2.0).abs() < 1e-5, "{}", steep.sample(1.0).x);
        // 第二帧的入切线 4：中点 = 1 - 4 × 2 × 1/8
        let incoming = channel(Property::Translation, Interpolation::CubicSpline, vec![0.0, 2.0], vec![v(0.0), v(0.0), v(0.0), v(4.0), v(2.0), v(0.0)]);
        assert!(incoming.sample(1.0).x.abs() < 1e-5, "{}", incoming.sample(1.0).x);
    }

    #[test]
    fn validation() {
        let node = |parent| Node { parent, rest: Trs::default() };
        let ok = Skeleton { nodes: vec![node(None), node(Some(0))], skins: vec![Skin { joints: vec![1], inverse_bind: vec![Mat4::IDENTITY] }] };
        assert!(ok.validate(&[]).is_ok());
        assert!(Skeleton { nodes: vec![node(Some(0))], ..Default::default() }.validate(&[]).is_err(), "父节点要在前面");
        assert!(Skeleton { skins: vec![Skin { joints: vec![2], inverse_bind: vec![Mat4::IDENTITY] }], ..ok.clone() }.validate(&[]).is_err());
        assert!(Skeleton { skins: vec![Skin { joints: vec![1], inverse_bind: vec![] }], ..ok.clone() }.validate(&[]).is_err());
        let clip = |channel: Channel| [Animation { name: "a".into(), duration: 1.0, channels: vec![channel] }];
        let good = channel(Property::Scale, Interpolation::Linear, vec![0.0, 1.0], vec![Vec4::ONE; 2]);
        assert!(ok.validate(&clip(good.clone())).is_ok());
        assert!(ok.validate(&clip(Channel { node: 2, ..good.clone() })).is_err());
        assert!(ok.validate(&clip(Channel { times: vec![], values: vec![], ..good.clone() })).is_err());
        assert!(ok.validate(&clip(Channel { values: vec![Vec4::ONE], ..good.clone() })).is_err());
        assert!(ok.validate(&clip(Channel { times: vec![1.0, 0.0], ..good.clone() })).is_err());
        assert!(ok.validate(&clip(Channel { times: vec![0.0, f32::NAN], ..good.clone() })).is_err());
        assert!(ok.validate(&clip(Channel { interpolation: Interpolation::CubicSpline, ..good.clone() })).is_err(), "三次样条每帧三个值");
    }

    /// 一个节点，动画 slide 在 1 秒里把它从 x = 0 线性移到 x = 10。
    fn sliding_app(animator: Animator) -> (nge_ecs::App, Entity) {
        use crate::{Model, Render3dPlugin, Transform3d};

        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let slide = channel(Property::Translation, Interpolation::Linear, vec![0.0, 1.0], vec![Vec4::ZERO, Vec4::new(10.0, 0.0, 0.0, 0.0)]);
        let model = Model {
            skeleton: Skeleton { nodes: vec![Node { parent: None, rest: Trs { translation: Vec3::Y, ..Default::default() } }], skins: vec![] },
            animations: vec![Animation { name: "slide".into(), duration: 1.0, channels: vec![slide] }],
            ..Default::default()
        };
        let model = app.world_mut().resource_mut::<Assets3d>().add_model(model).unwrap();
        let entity = app.world_mut().spawn((Model3d(model), Transform3d::default(), animator)).id();
        (app, entity)
    }

    /// 推进 `dt` 秒，返回节点的 x 和动画时间。
    fn step(app: &mut nge_ecs::App, entity: Entity, dt: f32) -> (f32, f32) {
        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, dt);
        let world = app.world();
        (world.get::<Pose>(entity).unwrap().nodes[0].w_axis.x, world.get::<Animator>(entity).unwrap().time)
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

    /// 两节的棍子：关节 1 挂在关节 0 上方 1 处，绕 z 转 90° 后，绑定时在 (0, 2) 的点到了 (-1, 1)。
    #[test]
    fn joints_bend_bound_vertices() {
        let skeleton = Skeleton {
            nodes: vec![
                Node { parent: None, rest: Trs::default() },
                Node { parent: Some(0), rest: Trs { translation: Vec3::Y, ..Default::default() } },
            ],
            skins: vec![Skin { joints: vec![0, 1], inverse_bind: vec![Mat4::IDENTITY, Mat4::from_translation(Vec3::NEG_Y)] }],
        };
        let mut joints = Vec::new();
        skeleton.joints(0, &skeleton.rest(), &mut joints);
        assert!(joints.iter().all(|j| j.abs_diff_eq(Mat4::IDENTITY, 1e-6)), "不播动画时关节矩阵是单位阵");
        let bent = [Trs::default(), Trs { translation: Vec3::Y, rotation: Quat::from_rotation_z(FRAC_PI_2), ..Default::default() }];
        let mut globals = Vec::new();
        skeleton.globals(&bent, &mut globals);
        joints.clear();
        skeleton.joints(0, &globals, &mut joints);
        assert!((joints[1].transform_point3(Vec3::new(0.0, 2.0, 0.0)) - Vec3::new(-1.0, 1.0, 0.0)).length() < 1e-5);
        // 父节点转了 90°：子节点的平移也跟着转
        let turned = [Trs { rotation: Quat::from_rotation_z(FRAC_PI_2), ..Default::default() }, Trs { translation: Vec3::Y, ..Default::default() }];
        skeleton.globals(&turned, &mut globals);
        assert!((globals[1].w_axis.truncate() - Vec3::NEG_X).length() < 1e-5, "{}", globals[1].w_axis);
        skeleton.joints(5, &globals, &mut joints);
        assert_eq!(joints.len(), 2, "没有这套蒙皮时不加");
    }
}
