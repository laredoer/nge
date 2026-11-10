// 编辑器里实体画成什么（docs/editor.md）：用游戏里同样的颜色、尺寸、模型，但不要刚体、关节，不动。
import type { Entity } from "../../ts/level";
import type { LightNode, ModelNode, SpriteNode } from "../../ts/nge";
import { propOf, sizeOf, TILESET } from "./catalog";
import { COIN_SHADER } from "./effects";
import { crateAt, flagOn, goalAt, jellyAt, rampOf, swingAt, towerAt } from "./levels";

export interface Preview {
  sprites: SpriteNode[];
  models: ModelNode[];
  lights: LightNode[];
}

const TILE = TILESET.tile;
const still = ({ body: _body, chase: _chase, ...rest }: SpriteNode): SpriteNode => rest;
const fixed = ({ body: _body, trail: _trail, attach: _attach, ...rest }: ModelNode): ModelNode => rest;
const empty = (): Preview => ({ sprites: [], models: [], lights: [] });
/** 预览节点的 id：带上实体的 id，一个实体可能有好几个节点。 */
export const previewId = (e: Entity, part = "") => `ed:${e.id}${part === "" ? "" : `:${part}`}`;

export function previewOf(e: Entity): Preview {
  const id = previewId(e);
  const [w, h] = sizeOf(e);
  const p = empty();
  switch (e.type) {
    case "spawn":
      p.sprites.push({ id, x: e.x, y: e.y, w, h, z: 5, color: "#ff7a59" });
      break;
    case "coin":
      p.sprites.push({ id, x: e.x, y: e.y, w: 14, z: 2, rotation: 45, color: "#ffd93d", unlit: true, ...COIN_SHADER });
      break;
    case "enemy": {
      const patrol = propOf<number>(e, "patrol") * TILE;
      // 巡逻线：从现在的位置往右
      p.sprites.push({ id: previewId(e, "path"), x: e.x + patrol / 2, y: e.y, w: patrol, h: 2, z: 3, color: "#9b59b680", unlit: true });
      p.sprites.push({ id, x: e.x, y: e.y, w, h, z: 4, color: "#9b59b6" });
      break;
    }
    case "ghost":
      p.sprites.push({ id, x: e.x, y: e.y, w, h, z: 4, color: "#c8d2ffd0", unlit: true });
      break;
    case "lift": {
      const [dx, dy] = [propOf<number>(e, "dx"), propOf<number>(e, "dy")];
      // 走到头的位置画个虚影
      p.sprites.push({ id: previewId(e, "to"), x: e.x + (dy === 0 ? dx : 0), y: e.y + dy, w, h: 16, z: 1, color: "#c0855250" });
      p.sprites.push({ id, x: e.x, y: e.y, w, h: 16, z: 1, color: "#c08552" });
      break;
    }
    case "crate": {
      const { model } = crateAt(e.x, e.y);
      p.models.push({ ...fixed(model), id, x: e.x, y: e.y });
      break;
    }
    case "swing": {
      const { sprites } = swingAt(e.x, e.y + h / 2);
      p.sprites.push(...sprites.map((s) => ({ ...still(s), id: previewId(e, s.id) })));
      break;
    }
    case "goal": {
      const goal = { ...still(goalAt(e.x, e.y)), id };
      p.sprites.push(goal);
      p.models.push({ ...flagOn(goal), id: previewId(e, "flag") });
      break;
    }
    case "ramp": {
      const down = propOf<boolean>(e, "down");
      p.sprites.push(still(rampOf(id, [e.x - w / 2, e.y + (down ? h : -h) / 2], [e.x + w / 2, e.y + (down ? -h : h) / 2])));
      break;
    }
    case "tower":
      p.models.push(...towerAt(e.x, 0).models.slice(1).map((m, i) => ({ ...fixed(m), id: previewId(e, String(i)) })));
      break;
    case "jelly":
      p.models.push({ ...fixed(jellyAt(e.x, false).model), id });
      break;
    case "light": {
      const color = propOf<string>(e, "color");
      p.sprites.push({ id, x: e.x, y: e.y, w: 12, z: 6, color, unlit: true, shader: "circle" });
      const spot = propOf<string>(e, "kind") === "spot";
      const light: LightNode = { id, kind: spot ? "spot" : "point", x: e.x, y: e.y, z: propOf<number>(e, "z"), color, intensity: propOf<number>(e, "intensity"), range: propOf<number>(e, "range") };
      p.lights.push(spot ? { ...light, angle: propOf<number>(e, "angle"), direction: [0, -0.5, -1] } : light);
      break;
    }
    case "model":
      p.models.push({ id, model: `asset:/@platformer/${propOf<string>(e, "model")}`, x: e.x, y: e.y, z: propOf<number>(e, "z"), scale: propOf<number>(e, "scale") });
      break;
    default:
      // 不认识的类型：洋红色的方块，一眼看得出来
      p.sprites.push({ id, x: e.x, y: e.y, w, h, z: 6, color: "#ff00ff", unlit: true });
  }
  return p;
}
