//! 单向平台：只挡从上面（碰撞体自己的 +y）来的。动态刚体靠 rapier 的接触修改钩子，角色控制器在查询时跳过（见 docs/gameplay2d.md）。

use std::collections::{HashMap, HashSet};

use rapier2d::parry::bounding_volume::{Aabb, BoundingVolume};
use rapier2d::parry::query::contact;
use rapier2d::prelude::{ActiveHooks, Collider, ColliderHandle, ContactModificationContext, PhysicsHooks, Pose, Shape};
use rapier2d::prelude::QueryFilter as RapierFilter;

use crate::character::CONTACT_GAP;
use crate::world::Physics;

/// 平台表面的法线和「从平台指向对方」差多少以内才挡（60°）：从侧面撞上来的不挡。
const ALLOWED: f32 = 0.5;
/// 角色的底比平台顶面低多少以内还算在上面（point）：控制器和地面之间本来就留着一点间隙。
const SLACK: f32 = 1.0;
/// 接触法线朝上多少算踩在上面（cos 45°）。
const SUPPORT: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// 接触对的状态（存在 rapier 的 manifold user_data 里）：还没定、挡、穿过。
const UNKNOWN: u32 = 0;
const ALLOWED_STATE: u32 = 1;
const FORBIDDEN: u32 = 2;

/// （刚体的碰撞体，单向平台的碰撞体）：正在往下穿的，分开之前不挡。
pub(crate) type Dropping = HashSet<(ColliderHandle, ColliderHandle)>;

pub(crate) fn is_one_way(c: &Collider) -> bool {
    c.active_hooks().contains(ActiveHooks::MODIFY_SOLVER_CONTACTS)
}

/// 形状在 `pose` 时整个在平台顶面以上（按平台自己的坐标）。
pub(crate) fn above(shape: &dyn Shape, pose: &Pose, platform: &Collider) -> bool {
    height_above(shape, pose, platform) >= -SLACK
}

/// 形状的底比平台顶面高多少（按平台自己的坐标，负的是压进去了）。
fn height_above(shape: &dyn Shape, pose: &Pose, platform: &Collider) -> f32 {
    let local = platform.position().inverse() * pose;
    shape.compute_aabb(&local).mins.y - platform.shape().compute_local_aabb().maxs.y
}

/// 角色查碰撞时算不算这个碰撞体：单向平台只在角色整个在它上面、又没在往下穿时算。
pub(crate) fn blocks(dropping: &Dropping, me: ColliderHandle, shape: &dyn Shape, pose: &Pose, handle: ColliderHandle, c: &Collider) -> bool {
    !is_one_way(c) || (!dropping.contains(&(me, handle)) && above(shape, pose, c))
}

/// 给 rapier 的钩子：动态刚体和单向平台之间的接触，从平台上面来的留着，别的去掉。
/// 一对一旦从下面、侧面穿进去，就一直穿到分开为止（和 rapier 的 `update_as_oneway_platform` 一样，只是按世界坐标的法线算，平台转了也对）。
pub(crate) struct Hooks<'a> {
    pub dropping: &'a Dropping,
}

impl PhysicsHooks for Hooks<'_> {
    fn modify_solver_contacts(&self, ctx: &mut ContactModificationContext) {
        let one_way = |h: ColliderHandle| ctx.colliders.get(h).is_some_and(is_one_way);
        let (platform, other, sign) = if one_way(ctx.collider1) {
            (ctx.collider1, ctx.collider2, 1.0)
        } else if one_way(ctx.collider2) {
            (ctx.collider2, ctx.collider1, -1.0)
        } else {
            return;
        };
        let (Some(plat), Some(them)) = (ctx.colliders.get(platform), ctx.colliders.get(other)) else { return };
        let up = plat.position().rotation * rapier2d::math::Vector::Y;
        let body = |h: Option<rapier2d::prelude::RigidBodyHandle>| h.and_then(|h| ctx.bodies.get(h)).map_or(rapier2d::math::Vector::ZERO, |b| b.linvel());
        let (vp, vo) = if sign > 0.0 { (body(ctx.rigid_body1), body(ctx.rigid_body2)) } else { (body(ctx.rigid_body2), body(ctx.rigid_body1)) };
        // 按几何算对方是不是整个在平台上面：manifold 的法线会沿用穿进去时的旧值
        let gap = height_above(them.shape(), them.position(), plat);
        let on_top = gap >= -SLACK;
        // 往平台那边（往下）走的才接：还在往上穿的不接，不然刚越过顶面又被按回去
        let falling = (vo - vp).dot(up) <= 0.0;
        let dropping = self.dropping.contains(&(other, platform));
        let Some(rigid) = ctx.rigid_mut() else { return };
        if dropping {
            rigid.solver_contacts.clear();
            *rigid.user_data = FORBIDDEN;
            return;
        }
        // rapier 的法线从 collider1 指向 collider2：换成从平台指向对方
        let normal = *rigid.normal * sign;
        let ok = normal.dot(up) >= ALLOWED;
        // 整个在上面时按顶面接：法线朝上、间隙是离顶面多高
        let (pose, top) = (*plat.position(), plat.shape().compute_local_aabb().maxs.y);
        let face_up = |rigid: &mut rapier2d::prelude::ModifiableManifold| {
            *rigid.normal = up * sign;
            for c in rigid.solver_contacts.iter_mut() {
                // 接触点挪到顶面上和对方的底上（rapier 按两个点的间隙算）
                let theirs = if sign > 0.0 { c.anchor2 } else { c.anchor1 };
                let x = pose.inverse_transform_point(theirs).x;
                let on_plat = pose.transform_point(rapier2d::math::Vector::new(x, top));
                let on_them = on_plat + up * gap;
                (c.anchor1, c.anchor2) = if sign > 0.0 { (on_plat, on_them) } else { (on_them, on_plat) };
                c.dist = gap;
            }
        };
        match *rigid.user_data {
            ALLOWED_STATE => {
                if rigid.solver_contacts.is_empty() {
                    *rigid.user_data = UNKNOWN;
                } else if !ok && on_top {
                    face_up(rigid);
                }
            }
            FORBIDDEN => {
                // 穿到整个在上面、往下落了才重新挡
                if on_top && falling {
                    *rigid.user_data = ALLOWED_STATE;
                    if !ok {
                        face_up(rigid);
                    }
                } else {
                    rigid.solver_contacts.clear();
                }
            }
            _ => {
                if ok {
                    *rigid.user_data = ALLOWED_STATE;
                } else {
                    rigid.solver_contacts.clear();
                    // 刚好碰在一个点上时法线可能是 0，下一步再定
                    if normal.length_squared() > 0.1 {
                        *rigid.user_data = FORBIDDEN;
                    }
                }
            }
        }
    }
}

impl Physics {
    /// 站在单向平台上的刚体或角色从它上面穿下去（角色按「下」加跳）：现在脚下贴着的单向平台，和紧挨着它们下面的单向平台，在分开之前都不挡它。
    /// 没贴着单向平台、脚下还踩着别的（实心的）东西、或者不是刚体时返回 false，什么都不记。
    pub fn drop_through(&mut self, entity: bevy_ecs::entity::Entity) -> bool {
        let Some(me) = self.entities.get(&entity).and_then(|h| h.collider) else { return false };
        let Some(col) = self.world.colliders.get(me) else { return false };
        let (pose, shape) = (*col.position(), col.shared_shape().clone());
        let mut filter = RapierFilter::default().exclude_sensors().exclude_collider(me);
        if let Some(body) = col.parent() {
            filter = filter.exclude_rigid_body(body);
        }
        let queries = self.world.query_pipeline_with_filter(filter);
        let mine = shape.compute_aabb(&pose);
        let mut platforms = Vec::new();
        for (h, c) in queries.intersect_aabb_conservative(mine.loosened(CONTACT_GAP)) {
            let Some(hit) = contact(&pose, &*shape, c.position(), c.shape(), CONTACT_GAP).ok().flatten() else { continue };
            if is_one_way(c) {
                // 站在上面的才算：头顶贴着平台底的不算
                if above(&*shape, &pose, c) {
                    platforms.push(h);
                }
            } else if !c.is_sensor() && -hit.normal1.y >= SUPPORT {
                // 脚下还踩着实心的：不穿，这一跳照常跳
                return false;
            }
        }
        // 紧挨着下面的单向平台（两行单向瓦片）一起穿
        let mut i = 0;
        while i < platforms.len() {
            let Some(bottom) = self.world.colliders.get(platforms[i]).map(|c| c.compute_aabb()) else { break };
            for (h, c) in queries.intersect_aabb_conservative(bottom.loosened(CONTACT_GAP)) {
                let b = c.compute_aabb();
                let under = b.maxs.y <= bottom.mins.y + CONTACT_GAP && b.mins.x < mine.maxs.x && b.maxs.x > mine.mins.x;
                if is_one_way(c) && under && !platforms.contains(&h) {
                    platforms.push(h);
                }
            }
            i += 1;
        }
        self.dropping.extend(platforms.iter().map(|&p| (me, p)));
        // 停在平台上的动态刚体可能睡着了：叫醒，不然接触不再算、钩子也不跑
        if let Some(b) = self.world.colliders.get(me).and_then(|c| c.parent()).and_then(|b| self.world.bodies.get_mut(b)) {
            b.wake_up(true);
        }
        if !platforms.is_empty()
            && let Some(m) = self.entities.get_mut(&entity).and_then(|h| h.mover.as_mut())
        {
            // 角色这就离地：加重力、不贴地、不跟着平台走
            (m.grounded, m.ground) = (false, None);
        }
        !platforms.is_empty()
    }

    /// 已经分开的（包围盒外扩一点也不相交了、或者删掉了）、角色已经站到别的东西上的，不再算在往下穿。
    pub(crate) fn forget_drops(&mut self) {
        // 角色查地面时不算正在穿的平台：着地了就是站在别的东西上
        let landed: HashSet<ColliderHandle> =
            self.entities.values().filter(|h| h.mover.as_ref().is_some_and(|m| m.grounded)).filter_map(|h| h.collider).collect();
        self.dropping.retain(|(a, _)| !landed.contains(a));
        let colliders = &self.world.colliders;
        let aabb = |h: ColliderHandle| -> Option<Aabb> { colliders.get(h).map(|c| c.compute_aabb()) };
        // 同一个刚体穿的几块平台合起来算（叠着的单向平台穿过上面那块时还没碰到下面那块）
        let mut spans: HashMap<ColliderHandle, Aabb> = HashMap::new();
        for &(a, b) in &self.dropping {
            if let Some(b) = aabb(b) {
                spans.entry(a).and_modify(|s| s.merge(&b)).or_insert(b);
            }
        }
        self.dropping.retain(|&(a, b)| aabb(b).is_some() && matches!((aabb(a), spans.get(&a)), (Some(a), Some(s)) if a.loosened(CONTACT_GAP).intersects(s)));
    }
}
