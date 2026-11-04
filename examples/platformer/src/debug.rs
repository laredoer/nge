//! 调试视图（按 Q 开关）：所有碰撞体的轮廓（[`PhysicsDebug`]），加上角色周围的空间查询——往前、往下各一条射线，
//! 身边一个重叠框（见 docs/physics2d.md「空间查询」、docs/gizmos.md）。

use bevy_ecs::prelude::*;
use nge_ecs::Input;
use nge_ecs::glam::{Vec2, Vec4};
use nge_physics2d::{Physics, PhysicsDebug, QueryFilter, Velocity};
use nge_render2d::{DrawAt, Gizmos, Transform2d};
use nge_scene::SceneId;

use crate::PLAYER;

const TOGGLE_KEY: &str = "KeyQ";
const FORWARD: f32 = 120.0;
const DOWN: f32 = 200.0;
const BOX_HALF: Vec2 = Vec2::new(70.0, 50.0);

const RED: Vec4 = Vec4::new(1.0, 0.25, 0.2, 1.0);
const YELLOW: Vec4 = Vec4::new(1.0, 0.85, 0.2, 1.0);
const GREEN: Vec4 = Vec4::new(0.3, 1.0, 0.4, 1.0);
const BLUE: Vec4 = Vec4::new(0.3, 0.6, 1.0, 1.0);

#[derive(Resource, Debug)]
pub(crate) struct DebugView {
    /// 角色朝哪边（1 右，-1 左）：停下来时保持上一次的方向。
    facing: f32,
}

impl Default for DebugView {
    fn default() -> Self {
        Self { facing: 1.0 }
    }
}

pub(crate) fn debug_view(
    input: Res<Input>,
    physics: Res<Physics>,
    mut view: ResMut<DebugView>,
    mut debug: ResMut<PhysicsDebug>,
    players: Query<(Entity, &SceneId, &Transform2d, &DrawAt, &Velocity)>,
    drawn: Query<&DrawAt>,
    mut gizmos: Gizmos,
) {
    if input.key_pressed(TOGGLE_KEY) {
        debug.enabled = !debug.enabled;
    }
    let Some((entity, _, t, at, v)) = players.iter().find(|(_, id, ..)| id.0 == PLAYER) else { return };
    if v.linear.x.abs() > 1.0 {
        view.facing = v.linear.x.signum();
    }
    if !debug.enabled {
        return;
    }
    // 查询用物理的真实位置，画在插值过的位置上，和画面里的角色贴着
    let offset = at.translation - t.translation;
    let me = QueryFilter::default().exclude(entity);
    let mut draw = Draw(&mut gizmos, offset);
    for (dir, length) in [(Vec2::X * view.facing, FORWARD), (Vec2::NEG_Y, DOWN)] {
        let from = t.translation;
        match physics.raycast(from, dir, length, me) {
            Some(hit) => {
                draw.line(from, hit.point, 2.0, RED);
                draw.line(hit.point, hit.point + hit.normal * 24.0, 2.0, GREEN);
                draw.rect(hit.point, Vec2::splat(8.0), YELLOW);
            }
            None => draw.line(from, from + dir * length, 1.0, RED.with_w(0.4)),
        }
    }
    draw.rect(t.translation, BOX_HALF * 2.0, BLUE.with_w(0.15));
    // 框里的金币、敌人、平台（sensor 也算）；地图的碰撞体没有 DrawAt，不标
    for e in physics.overlap_box(t.translation, BOX_HALF, me.with_sensors()) {
        if let Ok(other) = drawn.get(e) {
            draw.0.rect(other.translation, Vec2::splat(10.0), BLUE);
        }
    }
}

/// 按物理位置画，统一加上画面的插值偏移。
struct Draw<'a, 'w>(&'a mut Gizmos<'w>, Vec2);

impl Draw<'_, '_> {
    fn rect(&mut self, center: Vec2, size: Vec2, color: Vec4) {
        self.0.rect(center + self.1, size, color);
    }

    fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Vec4) {
        self.0.line(a + self.1, b + self.1, width, color);
    }
}
