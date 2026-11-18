//! 单向平台：只挡从上面（碰撞体自己的 +y）来的。动态刚体靠 rapier 的接触修改钩子，角色控制器在查询时跳过（见 docs/gameplay2d.md）。

use std::collections::HashSet;

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
    let local = platform.position().inverse() * pose;
    shape.compute_aabb(&local).mins.y >= platform.shape().compute_local_aabb().maxs.y - SLACK
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
        let Some(up) = ctx.colliders.get(platform).map(|c| c.position().rotation * rapier2d::math::Vector::Y) else { return };
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
        match *rigid.user_data {
            ALLOWED_STATE => {
                if rigid.solver_contacts.is_empty() {
                    *rigid.user_data = UNKNOWN;
                }
            }
            FORBIDDEN => {
                // 穿到整个在上面（不再压进去）才重新挡
                if ok && rigid.solver_contacts.iter().all(|c| c.dist > 0.0) {
                    *rigid.user_data = ALLOWED_STATE;
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
    /// 站在单向平台上的刚体或角色从它上面穿下去（角色按「下」加跳）：现在脚下贴着的单向平台在分开之前都不挡它。
    /// 没贴着单向平台（或者不是刚体）时返回 false。
    pub fn drop_through(&mut self, entity: bevy_ecs::entity::Entity) -> bool {
        let Some(me) = self.entities.get(&entity).and_then(|h| h.collider) else { return false };
        let Some(col) = self.world.colliders.get(me) else { return false };
        let (pose, shape) = (*col.position(), col.shared_shape().clone());
        let mut filter = RapierFilter::default().exclude_sensors().exclude_collider(me);
        if let Some(body) = col.parent() {
            filter = filter.exclude_rigid_body(body);
        }
        let queries = self.world.query_pipeline_with_filter(filter);
        let platforms: Vec<ColliderHandle> = queries
            .intersect_aabb_conservative(shape.compute_aabb(&pose).loosened(CONTACT_GAP))
            // 站在上面的才算：头顶贴着平台底的不算
            .filter(|(_, c)| is_one_way(c) && above(&*shape, &pose, c))
            .filter(|(_, c)| contact(&pose, &*shape, c.position(), c.shape(), CONTACT_GAP).ok().flatten().is_some())
            .map(|(h, _)| h)
            .collect();
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

    /// 已经分开的（包围盒外扩一点也不相交了、或者删掉了）不再算在往下穿。
    pub(crate) fn forget_drops(&mut self) {
        let colliders = &self.world.colliders;
        let aabb = |h: ColliderHandle| -> Option<Aabb> { colliders.get(h).map(|c| c.compute_aabb()) };
        self.dropping.retain(|&(a, b)| matches!((aabb(a), aabb(b)), (Some(a), Some(b)) if a.loosened(CONTACT_GAP).intersects(&b)));
    }
}
