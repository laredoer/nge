// 3D 试验场（按 G 进出：controls.json 里的 lab 动作，见 docs/gameplay3d.md）：角色控制器、铰链门、链子、凸包石头、台阶的 hulls、斜坡的 trimesh、移动平台。
// 布置在 lab.json 里（Rust 测试也读它）；走、跳、相机、平台来回挪在 src/lab.rs。
import type { Joint3dNode, ModelNode, Scene } from "../../ts/nge";
import lab from "./lab.json";

/** 场景的 `level`：进出试验场都整个重建。 */
export const LAB_LEVEL = "lab3d";
export const LAB_HERO = lab.hero.id;

export function labScene(): Scene {
  return {
    level: LAB_LEVEL,
    background: "#1d2533",
    // 相机由 Rust 每帧跟着角色摆，这里只是进场时的位置
    camera3d: { x: 0, y: 240, z: 720, target: [0, 0, 300], fov: 55, shadowDistance: 1600 },
    lights: [{ id: "lab-sun", kind: "directional", direction: [-1, -2, -1.2], shadows: true }],
    ambient: { brightness: 0.4 },
    physics3d: { gravity: [0, -981, 0] },
    models: [lab.hero as ModelNode, ...(lab.models as ModelNode[])],
    joints3d: lab.joints3d as Joint3dNode[],
  };
}

/** 试验场里怎么处理一个动作：`toggle` 进出试验场（只在电脑上），`ignore` 不响应，`pass` 照常处理。
 *  试验场里只有 lab 管用：改键面板、换存档、语言、联网都不开（它们改的是关卡那边的状态）。 */
export function labAction(action: string, inLab: boolean, desktop: boolean): "toggle" | "ignore" | "pass" {
  if (action === "lab") return desktop ? "toggle" : "ignore";
  return inLab ? "ignore" : "pass";
}

/** 出试验场时去哪一关：进来前刚通关、还没换关（`cleared`）的去下一关，最后一关之后回第一关。 */
export function levelAfterLab(level: number, cleared: boolean, count: number): number {
  return cleared ? (level + 1) % count : level;
}
