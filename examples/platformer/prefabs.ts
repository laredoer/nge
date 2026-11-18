// 预制体和换关过场（见 docs/scene-flow.md）：背景的树丛、两种过场轮着用、像素风的主角图集不要 mipmap。
import type { Prefab, PrefabInstance, TextureOptions, TransitionNode } from "../../ts/nge";

/** 一棵树（树干加树冠），在 JSON 文件里，按路径实例化。 */
export const TREE = "asset:/@platformer/tree.json";

/** 树丛：两棵树，第二棵靠右 127、树冠高 14（以前单双号的树冠就是一高一低）。 */
export const GROVE: Prefab = {
  instances: [
    { id: "a", prefab: TREE },
    { id: "b", prefab: TREE, x: 127, set: { crown: { y: 104 } } },
  ],
};

export const PREFABS: Record<string, Prefab> = { grove: GROVE };

/** 远处的 12 丛树，跟以前一棵棵摆的 24 棵位置一样。 */
export const GROVES: PrefabInstance[] = Array.from({ length: 12 }, (_, k) => ({ id: `grove-${k}`, prefab: "grove", x: -600 + k * 220 + (k % 2) * 34 }));

/** 换关过场：偶数关淡黑、奇数关擦除；先把金币和通关的音效也载好。 */
export function levelTransition(index: number, sounds: string[]): TransitionNode {
  return { kind: index % 2 === 0 ? "fade" : "wipe", out: 0.35, in: 0.35, min: 0.8, preload: sounds };
}

/**
 * 主角的图集和地面的瓦片集都是像素风的一格格小图：mip 链按整张图算，缩小时相邻格子的颜色会混进来（格子边上出现杂色的缝），
 * 像素风缩小也该保持硬边。
 */
export const TEXTURES: Record<string, TextureOptions> = {
  "asset:/@platformer/hero.png": { mipmaps: false },
  "asset:/@platformer/tiles.png": { mipmaps: false },
};
