//! 按状态机（`nge_anim::AnimGraph`）的输出算 3D 姿势：每层的片段加权混合，覆盖层按遮罩盖上去，叠加层按参考姿势算差值叠上去（见 docs/anim-graph.md「3D」）。

use std::collections::HashSet;

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3, Vec4};
use nge_anim::{AnimEvent, AnimGraph, Graph, LayerOutput};
use nge_ecs::Time;

use crate::animation::{Skeleton, Trs};
use crate::animator::{Pose, apply, reset};
use crate::assets::{Assets3d, Model};
use crate::mesh::MAX_MORPH_TARGETS;
use crate::renderer::Model3d;

type Weights = [f32; MAX_MORPH_TARGETS];

/// 骨骼遮罩：`names` 里的节点和它们所有子孙是 1，别的是 0。第二个值是有没有对上任何一个名字。
pub fn mask(skeleton: &Skeleton, names: &[String]) -> (Vec<f32>, bool) {
    let mut out = vec![0.0; skeleton.nodes.len()];
    let mut matched = false;
    // 父在前：父节点已经算好
    for (i, node) in skeleton.nodes.iter().enumerate() {
        let named = skeleton.names.get(i).is_some_and(|n| !n.is_empty() && names.contains(n));
        matched |= named;
        let parent = node.parent.and_then(|p| out.get(p as usize).copied()).unwrap_or(0.0);
        out[i] = if named || parent > 0.0 { 1.0 } else { 0.0 };
    }
    (out, matched)
}

/// 加权混合几份姿势（权重加起来是 1）：平移、缩放、变形权重加权平均，旋转按和第一份同半球加权求和再归一化。
pub(crate) fn mix(poses: &[(&[Trs], &[Weights], f32)], locals: &mut Vec<Trs>, weights: &mut Vec<Weights>) {
    locals.clear();
    weights.clear();
    let Some(&(first, _, _)) = poses.first() else { return };
    for i in 0..first.len() {
        let (mut t, mut s, mut q, mut w, mut total) = (Vec3::ZERO, Vec3::ZERO, Vec4::ZERO, [0.0; MAX_MORPH_TARGETS], 0.0);
        let r0 = Vec4::from(first[i].rotation);
        for &(pose, ws, a) in poses.iter().filter(|p| p.2 > 0.0) {
            let x = pose[i];
            t += x.translation * a;
            s += x.scale * a;
            let r = Vec4::from(x.rotation);
            q += if r.dot(r0) < 0.0 { -r } else { r } * a;
            for (o, v) in w.iter_mut().zip(&ws[i]) {
                *o += v * a;
            }
            total += a;
        }
        if total > 0.0 {
            (t, s) = (t / total, s / total);
            w.iter_mut().for_each(|o| *o /= total);
        }
        locals.push(Trs { translation: t, rotation: crate::animation::rotation(q), scale: s });
        weights.push(w);
    }
}

/// 覆盖：每个节点按 `alpha`（层权重 × 遮罩）从下面混到 `top`。
pub(crate) fn overlay(base: &mut [Trs], base_w: &mut [Weights], top: &[Trs], top_w: &[Weights], alpha: &[f32]) {
    for (i, b) in base.iter_mut().enumerate() {
        let a = alpha.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let (Some(t), true) = (top.get(i), a > 0.0) else { continue };
        *b = Trs { translation: b.translation.lerp(t.translation, a), rotation: b.rotation.slerp(t.rotation, a), scale: b.scale.lerp(t.scale, a) };
        if let (Some(bw), Some(tw)) = (base_w.get_mut(i), top_w.get(i)) {
            for (o, v) in bw.iter_mut().zip(tw) {
                *o += (v - *o) * a;
            }
        }
    }
}

/// 叠加：`pose` 相对 `reference` 的差值按 `alpha` 叠到下面。参考的缩放某分量是 0 时那一分量按 1 算。
pub(crate) fn add(base: &mut [Trs], base_w: &mut [Weights], pose: &[Trs], pose_w: &[Weights], reference: &[Trs], ref_w: &[Weights], alpha: &[f32]) {
    for (i, b) in base.iter_mut().enumerate() {
        let a = alpha.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let (Some(p), Some(r), true) = (pose.get(i), reference.get(i), a > 0.0) else { continue };
        let delta = (r.rotation.inverse() * p.rotation).normalize();
        let r_scale = Vec3::select(r.scale.cmpeq(Vec3::ZERO), Vec3::ONE, r.scale);
        b.translation += (p.translation - r.translation) * a;
        b.rotation = (b.rotation * Quat::IDENTITY.slerp(delta, a)).normalize();
        b.scale *= Vec3::ONE.lerp(p.scale / r_scale, a);
        if let (Some(bw), Some(pw), Some(rw)) = (base_w.get_mut(i), pose_w.get(i), ref_w.get(i)) {
            for ((o, v), r) in bw.iter_mut().zip(pw).zip(rw) {
                *o += (v - r) * a;
            }
        }
    }
}

/// 片段 `clip` 在 `t` 的本地姿势；没有这个片段时是不播动画的姿势。
fn sample(model: &Model, clip: Option<&crate::Animation>, t: f32) -> (Vec<Trs>, Vec<Weights>) {
    let (mut locals, mut weights) = (Vec::new(), Vec::new());
    reset(model, &mut locals, &mut weights);
    if let Some(clip) = clip {
        apply(clip, t, &mut locals, &mut weights);
    }
    (locals, weights)
}

/// 几个 (片段名, 时间, 权重) 采样后混在一起；找不到的片段名记进 `missing`。
fn mix_clips(model: &Model, clips: impl Iterator<Item = (String, f32, f32)>, missing: &mut Vec<String>) -> (Vec<Trs>, Vec<Weights>) {
    let samples: Vec<_> = clips
        .map(|(name, t, w)| {
            let anim = model.animations.iter().find(|a| a.name == name);
            if anim.is_none() && !missing.contains(&name) {
                missing.push(name);
            }
            (sample(model, anim, t), w)
        })
        .collect();
    let refs: Vec<_> = samples.iter().map(|((l, w), a)| (&l[..], &w[..], *a)).collect();
    let (mut locals, mut weights) = (Vec::new(), Vec::new());
    mix(&refs, &mut locals, &mut weights);
    (locals, weights)
}

/// 按状态机的输出算本地姿势。返回模型里找不到的片段名、一个名字都没对上的遮罩所在的层。
pub(crate) fn graph_pose(model: &Model, graph: &Graph, out: &[LayerOutput], locals: &mut Vec<Trs>, weights: &mut Vec<Weights>) -> (Vec<String>, Vec<usize>) {
    let (mut missing, mut unmatched) = (Vec::new(), Vec::new());
    reset(model, locals, weights);
    let name = |c: usize| graph.clips()[c].name.clone();
    for (i, layer) in out.iter().enumerate() {
        if layer.clips.is_empty() {
            continue;
        }
        let (pose, pose_w) = mix_clips(model, layer.clips.iter().map(|c| (name(c.clip), c.time, c.weight)), &mut missing);
        // 第 0 层下面没东西：不管模式，直接是它
        if i == 0 {
            (*locals, *weights) = (pose, pose_w);
            continue;
        }
        if layer.weight <= 0.0 {
            continue;
        }
        let def = &graph.layers()[layer.layer];
        let alpha: Vec<f32> = match &def.mask {
            Some(names) => {
                let (m, matched) = mask(&model.skeleton, names);
                if !matched {
                    unmatched.push(layer.layer);
                }
                m.iter().map(|m| m * layer.weight).collect()
            }
            None => vec![layer.weight; locals.len()],
        };
        if layer.additive {
            // 参考姿势：reference 片段（或每个片段自己）的 0 秒，按同样的权重混
            let (reference, ref_w) = mix_clips(model, layer.clips.iter().map(|c| (name(def.reference.unwrap_or(c.clip)), 0.0, c.weight)), &mut missing);
            add(locals, weights, &pose, &pose_w, &reference, &ref_w, &alpha);
        } else {
            overlay(locals, weights, &pose, &pose_w, &alpha);
        }
    }
    (missing, unmatched)
}

/// 推进状态机、算姿势、发动画事件（`PostUpdate`，`nge_anim::Animated` 集）。没有 [`Pose`] 的补上。
#[allow(clippy::type_complexity)]
pub(crate) fn animate_graphs(
    mut commands: Commands,
    time: Res<Time>,
    assets: Res<Assets3d>,
    mut models: Query<(Entity, &Model3d, &mut AnimGraph, Option<&mut Pose>)>,
    mut events: MessageWriter<AnimEvent>,
    mut warned: Local<HashSet<(Entity, String)>>,
) {
    // 删掉的实体留下的提示记录：攒多了清一次
    if warned.len() > 1024 {
        warned.clear();
    }
    for (entity, model, mut graph, pose) in &mut models {
        let Some(m) = assets.model(model.0) else { continue };
        for fired in graph.step(time.delta, |name| m.animations.iter().find(|a| a.name == name).map(|a| a.duration)) {
            events.write(AnimEvent::new(entity, fired));
        }
        let (mut locals, mut weights) = (Vec::new(), Vec::new());
        let (missing, unmatched) = graph_pose(m, graph.graph(), graph.output(), &mut locals, &mut weights);
        for name in missing {
            if warned.insert((entity, name.clone())) {
                let names: Vec<&str> = m.animations.iter().map(|a| a.name.as_str()).collect();
                log::warn!("状态机用到的动画 {name:?} 模型里没有（有 {names:?}），按不播动画的姿势算");
            }
        }
        for layer in unmatched {
            if warned.insert((entity, format!("mask {layer}"))) {
                log::warn!("状态机第 {layer} 层的遮罩一个节点名都没对上（模型的节点：{:?}），这层不起作用", m.skeleton.names);
            }
        }
        let mut nodes = Vec::new();
        m.skeleton.globals(&locals, &mut nodes);
        match pose {
            Some(mut pose) => {
                (pose.nodes, pose.weights) = (nodes, weights);
            }
            None => {
                commands.entity(entity).insert(Pose::posed(nodes, weights));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use glam::{Mat4, Quat, Vec3, Vec4};
    use serde_json::json;

    use super::*;
    use crate::animation::{Animation, Channel, Interpolation, Node, Property};

    const NO_WEIGHTS: Weights = [0.0; MAX_MORPH_TARGETS];

    fn trs(t: Vec3, r: Quat, s: Vec3) -> Trs {
        Trs { translation: t, rotation: r, scale: s }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.abs_diff_eq(b, 1e-4)
    }

    fn same_rotation(a: Quat, b: Quat) -> bool {
        a.angle_between(b) < 1e-3
    }

    #[test]
    fn mix_averages_translation_scale_weights_and_rotation() {
        let a = [trs(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE)];
        let b = [trs(Vec3::new(10.0, 0.0, 0.0), Quat::from_rotation_z(FRAC_PI_2), Vec3::splat(3.0))];
        let (mut wa, mut wb) = ([NO_WEIGHTS], [NO_WEIGHTS]);
        wa[0][0] = 1.0;
        wb[0][0] = 0.0;
        wb[0][7] = 1.0;
        let (mut locals, mut weights) = (Vec::new(), Vec::new());
        mix(&[(&a, &wa, 0.75), (&b, &wb, 0.25)], &mut locals, &mut weights);
        assert!(near(locals[0].translation, Vec3::new(2.5, 0.0, 0.0)));
        assert!(near(locals[0].scale, Vec3::splat(1.5)));
        assert!((weights[0][0] - 0.75).abs() < 1e-5 && (weights[0][7] - 0.25).abs() < 1e-5, "{:?}", weights[0]);
        let half = mix_rotation(0.5);
        assert!(same_rotation(half, Quat::from_rotation_z(FRAC_PI_2 / 2.0)), "一半一半是 45°：{half}");
    }

    fn mix_rotation(w: f32) -> Quat {
        let a = [trs(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE)];
        // 故意给反号的四元数（同一个旋转）
        let b = [trs(Vec3::ZERO, -Quat::from_rotation_z(FRAC_PI_2), Vec3::ONE)];
        let (mut locals, mut weights) = (Vec::new(), Vec::new());
        mix(&[(&a, &[NO_WEIGHTS], 1.0 - w), (&b, &[NO_WEIGHTS], w)], &mut locals, &mut weights);
        locals[0].rotation
    }

    #[test]
    fn mix_handles_opposite_quaternion_signs() {
        assert!(same_rotation(mix_rotation(0.5), Quat::from_rotation_z(FRAC_PI_2 / 2.0)), "反号的不能抵消成 0");
        assert!(same_rotation(mix_rotation(1.0), Quat::from_rotation_z(FRAC_PI_2)));
    }

    fn skeleton() -> Skeleton {
        // 0 hips ─ 1 spine ─ 2 arm；3 leg 挂在 hips 上
        let node = |parent| Node { parent, rest: Trs::default() };
        Skeleton {
            nodes: vec![node(None), node(Some(0)), node(Some(1)), node(Some(0))],
            skins: vec![],
            names: ["hips", "spine", "arm", "leg"].map(String::from).to_vec(),
        }
    }

    #[test]
    fn mask_covers_the_named_nodes_and_their_children() {
        let s = skeleton();
        assert_eq!(mask(&s, &["spine".into()]), (vec![0.0, 1.0, 1.0, 0.0], true));
        assert_eq!(mask(&s, &["leg".into(), "arm".into()]), (vec![0.0, 0.0, 1.0, 1.0], true));
        assert_eq!(mask(&s, &["tail".into()]), (vec![0.0; 4], false), "一个都没对上");
        let unnamed = Skeleton { names: vec![], ..s };
        assert_eq!(mask(&unnamed, &["spine".into()]).1, false, "没有名字的模型");
    }

    #[test]
    fn overlay_follows_alpha_per_node() {
        let mut base = vec![trs(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE); 3];
        let mut base_w = vec![NO_WEIGHTS; 3];
        let top = vec![trs(Vec3::new(4.0, 0.0, 0.0), Quat::from_rotation_z(FRAC_PI_2), Vec3::splat(3.0)); 3];
        let mut top_w = vec![NO_WEIGHTS; 3];
        top_w.iter_mut().for_each(|w| w[0] = 1.0);
        overlay(&mut base, &mut base_w, &top, &top_w, &[0.0, 0.5, 1.0]);
        assert_eq!(base[0], Trs::default(), "遮罩外不动");
        assert!(near(base[1].translation, Vec3::new(2.0, 0.0, 0.0)) && near(base[1].scale, Vec3::splat(2.0)));
        assert!(same_rotation(base[1].rotation, Quat::from_rotation_z(FRAC_PI_2 / 2.0)));
        assert!(near(base[2].translation, Vec3::new(4.0, 0.0, 0.0)) && same_rotation(base[2].rotation, Quat::from_rotation_z(FRAC_PI_2)));
        assert_eq!((base_w[0][0], base_w[1][0], base_w[2][0]), (0.0, 0.5, 1.0));
    }

    #[test]
    fn add_applies_the_difference_from_the_reference() {
        let base_t = trs(Vec3::X, Quat::from_rotation_z(FRAC_PI_2), Vec3::splat(2.0));
        let mut base = vec![base_t; 2];
        let mut base_w = vec![NO_WEIGHTS; 2];
        base_w[0][0] = 0.5;
        let pose = vec![trs(Vec3::new(0.0, 2.0, 0.0), Quat::from_rotation_z(FRAC_PI_2 / 3.0), Vec3::new(3.0, 3.0, 5.0)); 2];
        let reference = vec![trs(Vec3::Y, Quat::IDENTITY, Vec3::new(1.5, 1.5, 0.0)); 2];
        let (mut pose_w, ref_w) = (vec![NO_WEIGHTS; 2], vec![NO_WEIGHTS; 2]);
        pose_w[0][0] = 0.2;
        add(&mut base, &mut base_w, &pose, &pose_w, &reference, &ref_w, &[1.0, 0.5]);
        assert!(near(base[0].translation, Vec3::new(1.0, 1.0, 0.0)), "加上 (0, 1, 0)：{}", base[0].translation);
        assert!(same_rotation(base[0].rotation, Quat::from_rotation_z(FRAC_PI_2 + FRAC_PI_2 / 3.0)), "90° 再转 30°");
        assert!(near(base[0].scale, Vec3::new(4.0, 4.0, 10.0)), "乘 2；参考是 0 的分量按 1：{}", base[0].scale);
        assert!((base_w[0][0] - 0.7).abs() < 1e-5, "变形权重加差值");
        assert!(near(base[1].translation, Vec3::new(1.0, 0.5, 0.0)), "一半");
        assert!(same_rotation(base[1].rotation, Quat::from_rotation_z(FRAC_PI_2 + FRAC_PI_2 / 6.0)));
        assert!(near(base[1].scale, Vec3::new(3.0, 3.0, 6.0)), "{}", base[1].scale);

        // 差值是 0（姿势就是参考）时什么都不变
        let mut same = vec![base_t];
        add(&mut same, &mut [NO_WEIGHTS], &pose[..1], &pose_w[..1], &pose[..1], &pose_w[..1], &[1.0]);
        assert!(near(same[0].translation, base_t.translation) && same_rotation(same[0].rotation, base_t.rotation) && near(same[0].scale, base_t.scale));
    }

    // ---------- 系统 ----------

    fn hold(node: u32, property: Property, value: Vec4) -> Channel {
        Channel { node, property, interpolation: Interpolation::Step, times: vec![0.0], values: vec![value] }
    }

    fn linear(node: u32, property: Property, from: Vec4, to: Vec4) -> Channel {
        Channel { node, property, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: vec![from, to] }
    }

    /// hips（0）下面挂 arm（1，本地 y = 1）。slide：hips 的 x 在 1 秒里 0 → 10；lift：arm 停在 y = 5；bob：hips 的 y 0 → 2；wave（0.5 秒）：0.1 秒处有事件。
    fn app(def: serde_json::Value) -> (nge_ecs::App, Entity) {
        use crate::{MaterialId, Mesh, Part, Render3dPlugin, Transform3d};
        let mut app = nge_ecs::App::new();
        app.add_plugins(Render3dPlugin);
        let mut assets = app.world_mut().resource_mut::<Assets3d>();
        let mesh = assets.add_mesh(Mesh::cube(1.0)).unwrap();
        let clip = |name: &str, duration: f32, channels| Animation { name: name.into(), duration, channels };
        let model = Model {
            parts: vec![Part { node: Some(1), ..Part::new(mesh, MaterialId::DEFAULT, Mat4::IDENTITY) }],
            skeleton: Skeleton {
                nodes: vec![Node { parent: None, rest: Trs::default() }, Node { parent: Some(0), rest: Trs { translation: Vec3::Y, ..Default::default() } }],
                skins: vec![],
                names: vec!["hips".into(), "arm".into()],
            },
            animations: vec![
                clip("slide", 1.0, vec![linear(0, Property::Translation, Vec4::ZERO, Vec4::new(10.0, 0.0, 0.0, 0.0))]),
                clip("lift", 1.0, vec![hold(1, Property::Translation, Vec4::new(0.0, 5.0, 0.0, 0.0))]),
                clip("bob", 1.0, vec![linear(0, Property::Translation, Vec4::ZERO, Vec4::new(0.0, 2.0, 0.0, 0.0))]),
                clip("wave", 0.5, vec![]),
            ],
        };
        let model = assets.add_model(model).unwrap();
        let graph = AnimGraph::from_def(&serde_json::from_value(def).unwrap()).unwrap();
        let entity = app.world_mut().spawn((Model3d(model), Transform3d::default(), graph)).id();
        (app, entity)
    }

    fn tick(app: &mut nge_ecs::App, dt: f32) {
        app.tick(nge_core::Viewport { size: [10.0, 10.0], pixels: [10, 10] }, dt);
    }

    fn node(app: &nge_ecs::App, entity: Entity, n: usize) -> Vec3 {
        app.world().get::<Pose>(entity).unwrap().nodes[n].w_axis.truncate()
    }

    #[test]
    fn layers_mask_and_additive_shape_the_pose() {
        let (mut app, e) = app(json!({
            "params": { "bob": 0.5 },
            "states": { "s": { "clip": "slide" } },
            "layers": [
                { "name": "upper", "mask": ["arm"], "weight": 0.5, "states": { "l": { "clip": "lift" } } },
                { "name": "bob", "mode": "additive", "weight": "bob", "states": { "b": { "clip": "bob" } } },
            ],
        }));
        tick(&mut app, 0.0);
        tick(&mut app, 0.5);
        // hips：slide 的 x = 5，bob 叠一半：(2 × 0.5 − 0) × 0.5 = 0.5
        assert!(near(node(&app, e, 0), Vec3::new(5.0, 0.5, 0.0)), "{}", node(&app, e, 0));
        // arm：遮罩里的 lift（本地 y = 5）按层权重一半盖本地 y = 1，得 3，在 hips 下面
        assert!(near(node(&app, e, 1), Vec3::new(5.0, 3.5, 0.0)), "{}", node(&app, e, 1));
    }

    #[test]
    fn graph_takes_over_from_animator_and_sends_events() {
        let (mut app, e) = app(json!({
            "states": { "w": { "clip": "wave" } },
            "clips": { "wave": { "events": [{ "t": 0.1, "name": "hand" }] } },
        }));
        app.world_mut().entity_mut(e).insert(crate::Animator::new("slide"));
        app.init_resource::<Seen>();
        app.add_systems(nge_ecs::PostUpdate, (|mut r: MessageReader<AnimEvent>, mut seen: ResMut<Seen>| seen.0.extend(r.read().cloned())).after(nge_anim::Animated));
        tick(&mut app, 0.0);
        tick(&mut app, 0.2);
        assert!(near(node(&app, e, 0), Vec3::ZERO), "有状态机时 Animator 不管：{}", node(&app, e, 0));
        let seen = &app.world().resource::<Seen>().0;
        assert_eq!(seen.iter().map(|ev| (ev.entity, ev.name.as_str(), ev.clip.as_str(), ev.state.as_str())).collect::<Vec<_>>(), [(e, "hand", "wave", "w")]);
        tick(&mut app, 0.2);
        assert_eq!(app.world().resource::<Seen>().0.len(), 1, "不重复");
    }

    #[derive(Resource, Default)]
    struct Seen(Vec<AnimEvent>);

    #[test]
    fn missing_clips_rest_and_unmatched_masks_are_reported() {
        let (mut app, e) = app(json!({ "states": { "s": { "clip": "nope" } }, "layers": [{ "mask": ["tail"], "states": { "l": { "clip": "lift" } } }] }));
        tick(&mut app, 0.0);
        tick(&mut app, 0.5);
        assert!(near(node(&app, e, 1), Vec3::Y), "找不到的片段停在不播动画的姿势，遮罩没对上的层不起作用：{}", node(&app, e, 1));
        let world = app.world();
        let model = world.resource::<Assets3d>().model(world.get::<Model3d>(e).unwrap().0).unwrap().clone();
        let graph = world.get::<AnimGraph>(e).unwrap();
        let (missing, unmatched) = graph_pose(&model, graph.graph(), graph.output(), &mut Vec::new(), &mut Vec::new());
        assert_eq!((missing, unmatched), (vec!["nope".to_owned()], vec![1]));
    }
}
