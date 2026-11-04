//! 空间查询的可视化（HUD 的「查询」按钮打开）：一条扫来扫去的射线、一个左右扫的框、一个绕圈的点，
//! 每帧按 `Physics` 的查询结果画出来（见 docs/physics2d.md「空间查询」）。

use std::f32::consts::FRAC_PI_2;

use bevy_ecs::prelude::*;
use nge_core::PropKey;
use nge_ecs::Time;
use nge_ecs::glam::{Vec2, Vec4};
use nge_physics2d::{Physics, QueryFilter};
use nge_render2d::{DrawAt, Sprite, TextureId, Transform2d};
use serde::Deserialize;

/// TSX 的 `queries` prop：开着时画查询。
pub const SHOW_QUERIES: PropKey<ShowQueries> = PropKey::new("queries");

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ShowQueries(pub bool);

/// 这一帧画的标记，下一帧删掉重画。
#[derive(Component)]
pub struct Overlay;

/// 射线从这里往下扫。
const RAY_ORIGIN: Vec2 = Vec2::new(0.0, 260.0);
const RAY_LENGTH: f32 = 1000.0;
/// 框在地面上方左右扫，点绕着这个圆心转。
const BOX_Y: f32 = -190.0;
const BOX_HALF: Vec2 = Vec2::new(60.0, 50.0);
const BOX_RANGE: f32 = 240.0;
const POINT_CENTER: Vec2 = Vec2::new(0.0, -60.0);
const POINT_RADIUS: Vec2 = Vec2::new(200.0, 120.0);
const Z: f32 = 100.0;

const RED: Vec4 = Vec4::new(1.0, 0.25, 0.2, 1.0);
const YELLOW: Vec4 = Vec4::new(1.0, 0.85, 0.2, 1.0);
const GREEN: Vec4 = Vec4::new(0.3, 1.0, 0.4, 1.0);
const BLUE: Vec4 = Vec4::new(0.3, 0.6, 1.0, 1.0);
const MAGENTA: Vec4 = Vec4::new(1.0, 0.3, 0.9, 1.0);

pub fn show_queries(
    time: Res<Time>,
    show: Res<ShowQueries>,
    physics: Res<Physics>,
    drawn: Query<&DrawAt>,
    old: Query<Entity, With<Overlay>>,
    mut commands: Commands,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    if !show.0 {
        return;
    }
    // 真实时间：暂停时也在扫，看得出查询还能用
    let t = time.elapsed as f32;
    let mut draw = Draw(&mut commands);

    let dir = Vec2::from_angle(-FRAC_PI_2 + 1.1 * (t * 0.6).sin());
    match physics.raycast(RAY_ORIGIN, dir, RAY_LENGTH, QueryFilter::default()) {
        Some(hit) => {
            draw.line(RAY_ORIGIN, hit.point, 2.0, RED);
            draw.line(hit.point, hit.point + hit.normal * 30.0, 2.0, GREEN);
            draw.rect(hit.point, Vec2::splat(8.0), YELLOW);
        }
        None => draw.line(RAY_ORIGIN, RAY_ORIGIN + dir * RAY_LENGTH, 1.0, RED.with_w(0.4)),
    }

    let center = Vec2::new(BOX_RANGE * (t * 0.4).sin(), BOX_Y);
    draw.rect(center, BOX_HALF * 2.0, BLUE.with_w(0.2));
    for e in physics.overlap_box(center, BOX_HALF, QueryFilter::default()) {
        if let Ok(at) = drawn.get(e) {
            draw.rect(at.translation, Vec2::splat(10.0), BLUE);
        }
    }

    let point = POINT_CENTER + POINT_RADIUS * Vec2::from_angle(t * 0.9);
    draw.line(point - Vec2::X * 8.0, point + Vec2::X * 8.0, 2.0, Vec4::ONE);
    draw.line(point - Vec2::Y * 8.0, point + Vec2::Y * 8.0, 2.0, Vec4::ONE);
    for e in physics.overlap_point(point, QueryFilter::default()) {
        if let Ok(at) = drawn.get(e) {
            draw.rect(at.translation, Vec2::splat(14.0), MAGENTA);
        }
    }
}

struct Draw<'a, 'w, 's>(&'a mut Commands<'w, 's>);

impl Draw<'_, '_, '_> {
    fn rect(&mut self, center: Vec2, size: Vec2, color: Vec4) {
        self.rotated(center, size, 0.0, color);
    }

    fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Vec4) {
        let d = b - a;
        self.rotated((a + b) / 2.0, Vec2::new(d.length(), width), d.to_angle(), color);
    }

    fn rotated(&mut self, center: Vec2, size: Vec2, rotation: f32, color: Vec4) {
        let sprite = Sprite { color, ..Sprite::from_texture(TextureId::WHITE).with_size(size) };
        self.0.spawn((Overlay, sprite, Transform2d { translation: center, rotation, z: Z, ..Default::default() }));
    }
}
