// 3D 试验场（按 G 进出：controls.json 里的 lab 动作，见 docs/gameplay3d.md）：角色控制器、铰链门、链子、凸包石头、台阶的 hulls、斜坡的 trimesh、移动平台。
// 画质（见 docs/render3d-quality.md）：程序天空和环境光照、距离雾加高度雾、PCSS 软阴影、三段级联带过渡、
// 链子边上一盏投影子的点光源、照着移动平台的一盏投影子的聚光灯；按 C 看级联染色。
// 布置在 lab.json 里（Rust 测试也读它）；走、跳、相机、平台来回挪、C 键在 src/lab.rs。
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
    camera3d: { x: 0, y: 240, z: 720, target: [0, 0, 300], fov: 55, shadowCascades: 3, shadowDistance: 1600 },
    lights: [
      { id: "lab-sun", kind: "directional", direction: [-1, -2, -1.2], shadows: true },
      { id: "lab-lamp", kind: "point", x: -180, y: 150, z: -250, color: "#ffc27a", intensity: 40000, range: 500, shadows: true },
      { id: "lab-spot", kind: "spot", x: 150, y: 320, z: -330, direction: [0, -1, -0.2], color: "#9fd0ff", intensity: 90000, range: 700, angle: 32, shadows: true, shadowPriority: 1 },
    ],
    // 环境光照接了天空，常量环境光调低
    ambient: { brightness: 0.1 },
    sky: { top: "#3a6bb5", horizon: "#c9d6e3", ground: "#4a4038", sunSize: 2 },
    fog: { start: 700, end: 3000, color: "#c9d6e3", sky: 0.6, max: 0.85, height: { base: 0, falloff: 0.02, density: 0.0008 } },
    shadows: { filter: "pcss", sunSize: 1.5, softness: 2, blend: 0.15, maxLights: 2 },
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
