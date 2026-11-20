//! 3D 导航网格与寻路代理：`scene.navmesh3d` 从场景地形模型与静态碰撞体自动构建 [`NavMesh3d`]，
//! `ModelNode.nav` 驱动 3D NPC / 伙伴沿导航网格平滑绕障跟随、巡逻或前往目标点，并支持 `Gizmos3d` 路径调试可视化。

use std::collections::HashMap;

use bevy_ecs::prelude::*;
use glam::{Quat, Vec2, Vec3, Vec4};
use nge_ai::{NavConfig3d, NavMesh3d, NavObstacle3d};
use nge_ecs::Time;
use nge_render3d::{Assets3d, Gizmos3d, Model3d, ModelId, Transform3d};
use serde::Deserialize;

use crate::nodes::ModelNode;
use crate::physics3d::{Body3dKind, Shape3dName};
use crate::protocol::Scene;
use crate::sync::{Declared, SceneId};

/// `scene.navmesh3d`：自动从指定 3D 地面模型和静态碰撞体构建 3D 导航网格。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavMesh3dNode {
    /// 用于提取可行走表面的 3D 模型 id（如 `"terrain"`）。
    pub surface: String,
    /// 最大可行走坡度（度），默认 45。
    #[serde(default = "default_max_slope")]
    pub max_slope: f32,
    /// 最低可行走海拔（用于过滤海平面以下区域），默认不限制。
    #[serde(default = "default_min_y")]
    pub min_y: f32,
    /// 角色避障膨胀半径，默认 12。
    #[serde(default = "default_agent_radius")]
    pub agent_radius: f32,
    /// 是否绘制 3D 导航路径调试线（Gizmos3d）。
    #[serde(default)]
    pub debug: bool,
}

fn default_max_slope() -> f32 {
    45.0
}

fn default_min_y() -> f32 {
    -1e9
}

fn default_agent_radius() -> f32 {
    12.0
}

/// `ModelNode.nav`：3D 导航寻路代理配置（支持跟随目标模型、多点循环巡逻、或前往指定坐标）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavAgent3dNode {
    /// 跟随目标 3D 模型的 id。
    pub follow: Option<String>,
    /// 巡逻路点列表 `[[x, y, z], ...]`（按顺序循环巡逻）。
    #[serde(default)]
    pub patrol: Vec<[f32; 3]>,
    /// 单个目标世界坐标 `[x, y, z]`。
    pub target: Option<[f32; 3]>,
    /// 移动速度（世界单位 / 秒），默认 95。
    #[serde(default = "default_nav_speed")]
    pub speed: f32,
    /// 跟随模式下与目标保持的最小距离，默认 48。
    #[serde(default = "default_min_distance")]
    pub min_distance: f32,
    /// 到达路点的判定距离，默认 16。
    #[serde(default = "default_arrive")]
    pub arrive: f32,
    /// 离导航地面的垂直高度偏移，默认 0。
    #[serde(default)]
    pub hover: f32,
    /// 转向平滑速度（弧度 / 秒），默认 7。
    #[serde(default = "default_turn_speed")]
    pub turn_speed: f32,
    /// 巡逻模式在每个路点停留的秒数，默认 1.2。
    #[serde(default = "default_wait")]
    pub wait: f32,
}

fn default_nav_speed() -> f32 {
    95.0
}

fn default_min_distance() -> f32 {
    48.0
}

fn default_arrive() -> f32 {
    16.0
}

fn default_turn_speed() -> f32 {
    7.0
}

fn default_wait() -> f32 {
    1.2
}

/// 挂在 3D 模型实体上的运行时导航寻路代理状态。
#[derive(Component, Debug, Clone)]
pub struct NavAgent3d {
    pub cfg: NavAgent3dNode,
    pub path: Vec<Vec3>,
    pub waypoint: usize,
    pub replan_timer: f32,
    pub patrol_index: usize,
    pub wait_timer: f32,
}

impl NavAgent3d {
    pub fn new(cfg: NavAgent3dNode) -> Self {
        Self { cfg, path: Vec::new(), waypoint: 0, replan_timer: 0.0, patrol_index: 0, wait_timer: 0.0 }
    }
}

/// 记录当前已构建的 `NavMesh3d` 缓存键，避免每帧重复构图；切换 `debug` 不触发重建。
#[derive(Resource, Default)]
pub(crate) struct NavMesh3dCache {
    key: Option<(String, u32, u32, u32, ModelId)>,
}

/// 有 `nav` 的模型在声明出生点未变时保留当前寻路位置（避免切换 UI/天色时瞬移回出生点）。
pub(crate) fn keeps_place(before: Option<&ModelNode>, node: &ModelNode) -> bool {
    before.is_some_and(|b| b.nav.is_some() && node.nav.is_some() && (b.x, b.y, b.z) == (node.x, node.y, node.z))
}

/// 同步模型节点上的 `nav` 到 [`NavAgent3d`] 组件。
pub(crate) fn apply_nav(entity: &mut EntityCommands, before: Option<&ModelNode>, node: &ModelNode) {
    let placed = keeps_place(before, node);
    if placed && before.is_some_and(|b| b.nav == node.nav) {
        return;
    }
    match &node.nav {
        Some(cfg) => {
            if !cfg.speed.is_finite() || cfg.speed < 0.0 {
                log::warn!("模型 {:?} 的 nav.speed 不合法，跳过导航", node.id);
                entity.remove::<NavAgent3d>();
                return;
            }
            let cfg = cfg.clone();
            if placed {
                entity.queue(move |mut e: EntityWorldMut| match e.get_mut::<NavAgent3d>() {
                    Some(mut old) => {
                        old.cfg = cfg;
                        old.path.clear();
                        old.waypoint = 0;
                        old.replan_timer = 0.0;
                    }
                    None => {
                        e.insert(NavAgent3d::new(cfg));
                    }
                });
            } else {
                entity.insert(NavAgent3d::new(cfg));
            }
        }
        None => {
            entity.remove::<NavAgent3d>();
        }
    }
}

/// `Update`：当 `scene.navmesh3d` 指向的地面模型加载完毕且参数变化时，自动构建 [`NavMesh3d`]。
pub(crate) fn sync_navmesh3d(
    scene: Res<Scene>,
    assets: Res<Assets3d>,
    models: Query<(&SceneId, &Declared<ModelNode>, &Transform3d, Option<&Model3d>)>,
    mut navmesh: ResMut<NavMesh3d>,
    mut cache: ResMut<NavMesh3dCache>,
) {
    let Some(cfg) = &scene.navmesh3d else {
        if cache.key.is_some() {
            *navmesh = NavMesh3d::default();
            cache.key = None;
        }
        return;
    };
    let Some((_, _, surface_t, Some(surface_model))) = models.iter().find(|(id, ..)| id.0 == cfg.surface) else {
        return;
    };
    let next_key = (cfg.surface.clone(), cfg.max_slope.to_bits(), cfg.min_y.to_bits(), cfg.agent_radius.to_bits(), surface_model.0);
    if cache.key.as_ref() == Some(&next_key) {
        return;
    }
    let Some(model) = assets.model(surface_model.0) else { return };

    // 提取地面模型的全部世界空间三角形
    let base_mat = surface_t.matrix();
    let mut triangles = Vec::new();
    for part in &model.parts {
        let Some(mesh) = assets.mesh(part.mesh) else { continue };
        let m = base_mat * part.local;
        for tri in mesh.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            if i0 >= mesh.vertices.len() || i1 >= mesh.vertices.len() || i2 >= mesh.vertices.len() {
                continue;
            }
            let a = m.transform_point3(Vec3::from(mesh.vertices[i0].position));
            let b = m.transform_point3(Vec3::from(mesh.vertices[i1].position));
            let c = m.transform_point3(Vec3::from(mesh.vertices[i2].position));
            triangles.push([a, b, c]);
        }
    }

    // 自动收集场景中其他静态刚体（树干、巨石、圣坛、石柱等）作为障碍物
    let mut obstacles = Vec::new();
    for (id, declared, t, _) in &models {
        if id.0 == cfg.surface {
            continue;
        }
        let Some(body) = &declared.0.body else { continue };
        if body.kind != Body3dKind::Fixed {
            continue;
        }
        let scale = declared.0.scale.max(0.01);
        match body.shape {
            Shape3dName::Box => {
                let half = body.size.map_or(Vec3::splat(scale * 0.5), |s| Vec3::from(s) * 0.5);
                obstacles.push(NavObstacle3d::Box { min: t.translation - half, max: t.translation + half });
            }
            Shape3dName::Ball => {
                let r = body.r.unwrap_or(scale * 0.5);
                obstacles.push(NavObstacle3d::Cylinder { center: t.translation, radius: r, half_height: r });
            }
            Shape3dName::Capsule => {
                let r = body.r.unwrap_or(scale * 0.25);
                let hh = body.h.unwrap_or(scale * 0.5) * 0.5 + r;
                obstacles.push(NavObstacle3d::Cylinder { center: t.translation, radius: r, half_height: hh });
            }
            Shape3dName::Convex | Shape3dName::Hulls => {
                let r = 22.0 * scale;
                obstacles.push(NavObstacle3d::Cylinder { center: t.translation, radius: r, half_height: 24.0 * scale });
            }
            Shape3dName::TriMesh => {}
        }
    }

    let built = NavMesh3d::build(
        &triangles,
        &NavConfig3d { max_slope_deg: cfg.max_slope, min_y: cfg.min_y, agent_radius: cfg.agent_radius, obstacles },
    );
    log::info!("NavMesh3d 构建完成：{} 个可行走三角形（源模型 {:?}）", built.tri_count(), cfg.surface);
    *navmesh = built;
    cache.key = Some(next_key);
}

/// `Update`：推进所有 [`NavAgent3d`] 沿 [`NavMesh3d`] 寻路、贴地移动、平滑转向，并在开启 `debug` 时绘制路径。
pub(crate) fn step_nav_agents(
    time: Res<Time>,
    scene: Res<Scene>,
    navmesh: Res<NavMesh3d>,
    mut gizmos: ResMut<Gizmos3d>,
    targets: Query<(&SceneId, &Transform3d), Without<NavAgent3d>>,
    mut agents: Query<(&mut NavAgent3d, &mut Transform3d)>,
) {
    if navmesh.is_empty() || agents.is_empty() {
        return;
    }
    let dt = time.delta.min(0.1);
    let debug = scene.navmesh3d.as_ref().is_some_and(|n| n.debug);

    let target_positions: HashMap<&str, Vec3> = targets.iter().map(|(id, t)| (id.0.as_str(), t.translation)).collect();

    for (mut agent, mut transform) in &mut agents {
        let pos = transform.translation;
        let ground_pos = Vec3::new(pos.x, pos.y - agent.cfg.hover, pos.z);

        // 1. 解析当前目标坐标
        let (goal, stop_dist) = if let Some(target_id) = agent.cfg.follow.as_deref() {
            let Some(&target_pos) = target_positions.get(target_id) else { continue };
            (target_pos, agent.cfg.min_distance.max(agent.cfg.arrive))
        } else if !agent.cfg.patrol.is_empty() {
            let idx = agent.patrol_index % agent.cfg.patrol.len();
            let wp = Vec3::from(agent.cfg.patrol[idx]);
            let dist_xz = Vec2::new(wp.x - pos.x, wp.z - pos.z).length();
            if dist_xz <= agent.cfg.arrive {
                agent.path.clear();
                agent.wait_timer += dt;
                if agent.wait_timer >= agent.cfg.wait {
                    agent.wait_timer = 0.0;
                    agent.patrol_index = (idx + 1) % agent.cfg.patrol.len();
                    agent.replan_timer = 0.0;
                }
                continue;
            }
            (wp, agent.cfg.arrive)
        } else if let Some(t) = agent.cfg.target {
            (Vec3::from(t), agent.cfg.arrive)
        } else {
            continue;
        };

        // 2. 已在停止距离内：清空路径并柔和转向目标
        let to_goal_xz = Vec2::new(goal.x - pos.x, goal.z - pos.z);
        if to_goal_xz.length() <= stop_dist {
            agent.path.clear();
            agent.waypoint = 0;
            if to_goal_xz.length_squared() > 1e-4 {
                let desired = Quat::from_rotation_y((-to_goal_xz.x).atan2(-to_goal_xz.y));
                let factor = (agent.cfg.turn_speed * dt).clamp(0.0, 1.0);
                transform.rotation = transform.rotation.slerp(desired, factor);
            }
            if let Some(y) = navmesh.sample_height(Vec2::new(pos.x, pos.z), ground_pos.y) {
                transform.translation.y = y + agent.cfg.hover;
            }
            continue;
        }

        // 3. 定时重算路径（或路径耗尽时立即重算）
        agent.replan_timer -= dt;
        if agent.replan_timer <= 0.0 || agent.waypoint >= agent.path.len() {
            agent.replan_timer = 0.35;
            if let Some(path) = navmesh.find_path(ground_pos, goal) {
                agent.path = path;
                agent.waypoint = if agent.path.len() > 1 { 1 } else { 0 };
            }
        }

        // 4. 沿平滑路点前进并贴合 3D 地形高度
        let mut remaining = agent.cfg.speed * dt;
        let mut move_dir = Vec2::ZERO;
        while remaining > 1e-4 && agent.waypoint < agent.path.len() {
            let wp = agent.path[agent.waypoint];
            let delta = Vec2::new(wp.x - transform.translation.x, wp.z - transform.translation.z);
            let dist = delta.length();
            if dist <= remaining {
                transform.translation.x = wp.x;
                transform.translation.z = wp.z;
                remaining -= dist;
                if dist > 1e-4 {
                    move_dir = delta / dist;
                }
                agent.waypoint += 1;
            } else {
                let dir = delta / dist;
                transform.translation.x += dir.x * remaining;
                transform.translation.z += dir.y * remaining;
                move_dir = dir;
                remaining = 0.0;
            }
        }
        if let Some(y) = navmesh.sample_height(Vec2::new(transform.translation.x, transform.translation.z), ground_pos.y) {
            transform.translation.y = y + agent.cfg.hover;
        }
        if move_dir.length_squared() > 1e-4 {
            let desired = Quat::from_rotation_y((-move_dir.x).atan2(-move_dir.y));
            let factor = (agent.cfg.turn_speed * dt).clamp(0.0, 1.0);
            transform.rotation = transform.rotation.slerp(desired, factor);
        }

        // 5. 调试可视化：用 Gizmos3d 画出实时寻路折线与路点圆环
        if debug && agent.path.len() >= 2 {
            let color = if agent.cfg.follow.is_some() {
                Vec4::new(0.25, 0.95, 1.0, 1.0)
            } else {
                Vec4::new(1.0, 0.82, 0.25, 1.0)
            };
            let lift = Vec3::Y * 6.0;
            for seg in agent.path.windows(2) {
                gizmos.on_top().line(seg[0] + lift, seg[1] + lift, color);
            }
            for &wp in &agent.path {
                gizmos.on_top().circle(wp + lift, Vec3::Y, 6.0, color);
            }
        }
    }
}

#[cfg(all(test, feature = "ai"))]
mod tests {
    use nge_render3d::{Assets3d, MaterialId, Mesh, Model, Part, Vertex};
    use serde_json::json;

    use super::*;
    use crate::testing::{app, frame};

    /// 建一个 6×6 的平坦网格模型（x, z ∈ [0, 60]）。
    fn flat_ground_model(world: &mut World) -> ModelId {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = 6u32;
        let step = 10.0;
        for gz in 0..=n {
            for gx in 0..=n {
                vertices.push(Vertex { position: [gx as f32 * step, 0.0, gz as f32 * step], normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0] });
            }
        }
        let w = n + 1;
        for gz in 0..n {
            for gx in 0..n {
                let a = gz * w + gx;
                indices.extend([a, a + w, a + 1, a + 1, a + w, a + w + 1]);
            }
        }
        let mut assets = world.resource_mut::<Assets3d>();
        let mesh = assets.add_mesh(Mesh { vertices, indices, ..Default::default() }).unwrap();
        assets.add_model(Model { parts: vec![Part::new(mesh, MaterialId::DEFAULT, glam::Mat4::IDENTITY)], ..Default::default() }).unwrap()
    }

    #[test]
    fn scene_navmesh3d_builds_from_surface_and_guides_agent() {
        let mut app = app();
        let scene = json!({
            "navmesh3d": { "surface": "ground", "agentRadius": 4, "debug": true },
            "models": [
                { "id": "ground", "model": "asset:/@t/crate.glb" },
                { "id": "hero", "model": "asset:/@t/crate.glb", "x": 55, "y": 0, "z": 55 },
                { "id": "pet", "model": "asset:/@t/crate.glb", "x": 5, "y": 0, "z": 5, "nav": { "follow": "hero", "speed": 60, "minDistance": 8, "hover": 4 } },
            ]
        });
        frame(&mut app, Some(scene));
        // 模拟 ground 加载完成
        let world = app.world_mut();
        let model_id = flat_ground_model(world);
        let ground = world.query::<(Entity, &SceneId)>().iter(world).find(|(_, id)| id.0 == "ground").unwrap().0;
        world.entity_mut(ground).insert(Model3d(model_id));

        for _ in 0..25 {
            frame(&mut app, None);
        }
        assert!(app.world().resource::<NavMesh3d>().tri_count() > 0, "应自动构建出 3D 导航网格");
        let world = app.world_mut();
        let pet_pos = world.query::<(&SceneId, &Transform3d)>().iter(world).find(|(id, _)| id.0 == "pet").unwrap().1.translation;
        assert!(pet_pos.distance(Vec3::new(55.0, 4.0, 55.0)) <= 12.0, "伙伴应寻路走到目标身边：{pet_pos:?}");
    }
}
