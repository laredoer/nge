// 纵深带子（belt）和画面坐标的换算。
//
// 人、箭、物件都按「带子坐标」摆：x 左右，d 纵深（0 是离镜头最近的那条边，DEPTH 是最远的），h 离地多高（跳起来）。
// 换到画面：
//   - 脚底的 y = GROUND_Y + d + h：往里走一格，画面上抬高一格；
//   - 深度   z = Z_NEAR - d：往里走一格，离相机远一格（和 h 无关，跳起来不改前后）。
// 3D 物件（tools/make_models.py）按同样的剪切 (x, h + d, -d) 建模，放在 (x, footY(d0), zOf(d0))，
// 所以模型上每一点的 y、z 和站在同一纵深的精灵一致。3D 相机正交、正对 -Z（camera3d.match2d），
// 混排（interleave）时精灵的 z 当 3D 深度比：物件挡住纵深更大（更靠里）的人，被纵深更小的人挡住。
// 地面本身也是 3D 模型（顶面是倾斜 45° 的一片），所以能接住物件的影子。
// 精灵一律 z ≥ 0：z < 0 是背景层，背景层有后处理（模糊）时整层画在 3D 后面。

/** 纵深 0（最靠前）那条边上，脚底的 y。 */
export const GROUND_Y = -200;
/** 纵深 0 的 z；地面往前一直铺到 d = -320，z 也不超过相机的 1000。 */
export const Z_NEAR = 400;
/** 能走的纵深范围 [0, DEPTH]。 */
export const DEPTH = 130;
/** 画面一半宽（按 800 宽的窗口算）：镜头锁定、刷怪、人不能走出去都按它。 */
export const VIEW_HALF = 400;
/** 相机的 y（不上下动）。 */
export const CAMERA_Y = 0;

/** 站在纵深 d、离地 h 的人，脚底在画面上的 y。 */
export function footY(d: number, h = 0): number {
  return GROUND_Y + d + h;
}

/** 纵深 d 的深度（精灵的 z，也是 3D 的 z）。 */
export function zOf(d: number): number {
  return Z_NEAR - d;
}

/** 纵深 d 的人身上的东西往前挪一点（影子、人、特效），按 z 排的前后不变，又不至于和地面打架。 */
export const LAYER = { shadow: 2, body: 4, fx: 6 } as const;

export function clamp(v: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, v));
}
