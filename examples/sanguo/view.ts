// 把一局游戏画成场景（纯函数）：人和箭是图集精灵，刀光、烟是特效精灵；城墙、粮车、旌旗、拒马、桥、地面是 3D 模型。
// 前后关系全靠 belt.ts 的换算：精灵的 z 和模型的 z 按纵深一致，混排时按深度比。
import type { BarNode, ModelNode, Scene, SpriteNode } from "../../ts/nge";
import { CAMERA_Y, footY, LAYER, VIEW_HALF, zOf } from "./belt";
import { AMBIENT, BACKDROP, FLASH, fireEmitters, fireLights, fireLights2d, POST, SHADERS, SPRITE_AMBIENT, SUN } from "./effects";
import type { Enemy } from "./enemies";
import type { Fx, Game } from "./game";
import { BRIDGE_LEN, BRIDGE_X, GROUND_LEN, PROPS, WORLD, type Prop } from "./levels";
import { ATTACKS, PLUNGE, THRUST, type Player } from "./player";
import SHEETS from "./sheets.json";

export interface Sheet {
  texture: string;
  grid: number[];
  size: number[];
  foot: number;
  frames: Record<string, number[]>;
}

export const PLAYER_SHEET: Sheet = SHEETS.player;
export const TROOP_SHEET: Sheet = SHEETS.troops;
export const BOSS_SHEET: Sheet = SHEETS.boss;
export const FX_SHEET: Sheet = SHEETS.fx;

export const SOUNDS = {
  swing: "asset:/@sanguo/swing.wav",
  hit: "asset:/@sanguo/hit.wav",
  arrow: "asset:/@sanguo/arrow.wav",
  hooves: "asset:/@sanguo/hooves.wav",
  win: "asset:/@sanguo/win.wav",
  lose: "asset:/@sanguo/lose.wav",
};

/** 动作里的第几帧：按进度 0..1 平均分。 */
function pick(frames: number[], progress: number): number {
  return frames[Math.max(0, Math.min(frames.length - 1, Math.floor(progress * frames.length)))];
}

function loop(frames: number[], t: number, fps: number): number {
  return frames[Math.floor(t * fps) % frames.length];
}

export function playerFrame(p: Player, time: number): number {
  const f = PLAYER_SHEET.frames;
  switch (p.act) {
    case "walk":
      return loop(f.walk, p.t, 9);
    case "attack":
      return pick(f[`atk${p.step}`], p.t / ATTACKS[p.step - 1].dur);
    case "thrust":
      return pick(f.thrust, p.t / THRUST.dur);
    case "jump":
      return f.jump[p.vh > 0 ? 0 : 1];
    case "plunge":
      return f.plunge[p.t < 0.06 ? 0 : 1];
    case "land":
      return pick(f.plunge, 0.5 + p.t / PLUNGE.recover);
    case "hurt":
      return f.hurt[0];
    case "down":
      return f.down[p.t < 0.15 ? 0 : 1];
    case "dead":
      return f.down[p.t < 0.15 ? 0 : 1];
    default:
      return loop(f.idle, time, 2);
  }
}

export function enemyFrame(e: Enemy, time: number): number {
  const sheet = e.kind === "boss" ? BOSS_SHEET : TROOP_SHEET;
  const prefix = e.kind === "boss" ? "" : `${e.kind}.`;
  const f = (name: string) => sheet.frames[`${prefix}${name}`];
  switch (e.act) {
    case "walk":
      return loop(f("walk"), e.t, e.kind === "boss" ? 8 : 7);
    case "windup":
      return e.kind === "boss" ? f("attack")[0] : f("attack")[0];
    case "draw":
      return pick(f("attack").slice(0, 2), e.t / 0.65);
    case "attack":
      return f("attack")[1];
    case "recover":
      return f("attack")[2];
    case "chargeWindup":
      return pick(f("windup"), e.t / 0.6);
    case "charge":
      return loop(f("charge"), e.t, 10);
    case "hurt":
      return f("hurt")[0];
    case "launched":
      return f("hurt")[0];
    case "down":
    case "getup":
      return e.act === "getup" ? f("down")[0] : f("down")[e.t < 0.1 ? 0 : 1];
    case "dead":
      return f("down")[e.t < 0.15 ? 0 : 1];
    default:
      return loop(f("idle"), time + e.slot * 0.3, 2);
  }
}

/** 一个人：图集精灵（朝左时宽写成负的，镜像）和脚下的影子。跳起来影子变小，留在地上。 */
function actor(id: string, sheet: Sheet, frame: number, x: number, d: number, h: number, face: number, flash: boolean, shadow: number): SpriteNode[] {
  return [
    { id: `${id}-shadow`, x, y: footY(d) + 2, w: shadow * (1 - Math.min(h, 150) / 300), h: 12, z: zOf(d) + LAYER.shadow, color: "#000000a0", shader: "blob", unlit: true },
    {
      id, texture: sheet.texture, grid: sheet.grid as [number, number], frame, x, y: footY(d, h) + sheet.foot, z: zOf(d) + LAYER.body,
      w: face * sheet.size[0], h: sheet.size[1], ...(flash ? FLASH : {}),
    },
  ];
}

export function actorSprites(g: Game): SpriteNode[] {
  const p = g.player;
  // 无敌（受击后）时一闪一闪
  const blink = p.invuln > 0 && p.act !== "down" && Math.floor(g.time * 20) % 2 === 1;
  const sprites = actor("player", PLAYER_SHEET, playerFrame(p, g.time), p.x, p.d, p.h, p.face, blink, 56);
  for (const e of g.enemies) {
    const sheet = e.kind === "boss" ? BOSS_SHEET : TROOP_SHEET;
    sprites.push(...actor(e.id, sheet, enemyFrame(e, g.time), e.x, e.d, e.h, e.face, e.flash > 0, e.kind === "boss" ? 120 : 52));
  }
  return sprites;
}

export function fxSprites(g: Game): SpriteNode[] {
  const fx = g.fx.map((f: Fx): SpriteNode => ({
    id: f.id, texture: FX_SHEET.texture, grid: FX_SHEET.grid as [number, number], frame: pick(FX_SHEET.frames[f.name], f.t / f.dur),
    x: f.x, y: footY(f.d, f.h), z: zOf(f.d) + LAYER.fx, w: f.face * FX_SHEET.size[0], h: FX_SHEET.size[1], unlit: true,
  }));
  const arrows = g.arrows.map((a): SpriteNode => ({
    id: a.id, texture: FX_SHEET.texture, grid: FX_SHEET.grid as [number, number], frame: FX_SHEET.frames.arrow[0],
    x: a.x, y: footY(a.d, a.h), z: zOf(a.d) + LAYER.fx, w: Math.sign(a.vx) * FX_SHEET.size[0], h: FX_SHEET.size[1],
  }));
  return [...fx, ...arrows];
}

/** 小兵挨过打以后头顶的血条（首领的在 HUD 下方）。血条画在最前面（z 很大），不被物件挡。 */
export function enemyBars(g: Game): BarNode[] {
  return g.enemies
    .filter((e) => e.kind !== "boss" && !e.dead && e.hp < e.maxHp)
    .map((e) => ({ id: `${e.id}-hp`, follow: e.id, y: 46, w: 34, h: 4, value: e.hp / e.maxHp, color: "#e05040", z: 900, smooth: 0.1 }));
}

/** 模型放在纵深 d、x 处：y、z 按 belt.ts 换算，和站在同一纵深的精灵对齐。 */
export function modelAt(id: string, file: string, x: number, d: number, extra: Partial<ModelNode> = {}): ModelNode {
  return { id, model: `asset:/@sanguo/${file}.glb`, x, y: footY(d), z: zOf(d), ...extra };
}

const FILES: Record<Prop["kind"], string> = { wall: "wall", cart: "cart", fire: "cart_fire", banner: "banner", barricade: "barricade" };
/** 每种物件在 x 方向从原点往左、往右占多少（剔除用）。 */
const EXTENT: Record<Prop["kind"], [number, number]> = { wall: [-20, 280], cart: [-45, 80], fire: [-45, 80], banner: [-10, 60], barricade: [-25, 25] };
/** 镜头左右多远以内的模型才放进场景（影子图按放进来的模型拟合，少放一些更清楚）。 */
const MODEL_RANGE = VIEW_HALF + 300;

/** 地面一段段铺到桥头为止；桥（带河）；三段战场的物件。只放镜头附近的。 */
export function propModels(cam: number): ModelNode[] {
  const near = (x0: number, x1: number) => x1 > cam - MODEL_RANGE && x0 < cam + MODEL_RANGE;
  const models: ModelNode[] = [];
  for (let x = WORLD.left - GROUND_LEN; x < BRIDGE_X; x += GROUND_LEN) {
    if (near(x, x + GROUND_LEN)) models.push(modelAt(`ground-${x}`, "ground", x, 0));
  }
  if (near(BRIDGE_X - 300, BRIDGE_X + BRIDGE_LEN + 400)) models.push(modelAt("bridge", "bridge", BRIDGE_X, 0));
  for (const p of PROPS) {
    const [a, b] = EXTENT[p.kind];
    if (!near(p.x + a, p.x + b)) continue;
    models.push(modelAt(`${p.kind}-${p.x}-${p.d}`, FILES[p.kind], p.x, p.d, p.kind === "banner" ? { animation: { clip: "wave", speed: 0.8 + (p.x % 7) * 0.05 } } : {}));
  }
  return models;
}

/** 整个场景。`round` 换了值（重开）就把场景整个重建。 */
export function sceneOf(g: Game, paused: boolean, round: number): Scene {
  return {
    level: round,
    background: "#1d1424",
    timeScale: paused ? 0 : 1,
    shaders: SHADERS,
    post: POST,
    audio: { preload: Object.values(SOUNDS) },
    camera: { x: g.cam, y: CAMERA_Y },
    camera3d: { match2d: true, interleave: true },
    lights: [SUN, ...fireLights(g.cam)],
    ambient: AMBIENT,
    lighting: { ambient: SPRITE_AMBIENT, lights: fireLights2d(g.cam) },
    models: propModels(g.cam),
    emitters3d: fireEmitters(g.cam),
    sprites: [...BACKDROP, ...actorSprites(g), ...fxSprites(g)],
    bars: enemyBars(g),
  };
}
