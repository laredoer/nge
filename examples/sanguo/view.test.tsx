// 画面：纵深怎么换成 y 和 z（精灵和 3D 物件一致，前面的挡后面的）、图集的帧、镜像、闪白、只放镜头附近的模型。
import { expect, test } from "nsh/test";
import { DEPTH, footY, LAYER, zOf } from "./belt";
import { newEnemy } from "./enemies";
import { newGame, type Game } from "./game";
import { PROPS } from "./levels";
import { ATTACKS } from "./player";
import { actorSprites, BOSS_SHEET, enemyBars, enemyFrame, fxSprites, modelAt, PLAYER_SHEET, playerFrame, propModels, sceneOf, SOUNDS, TROOP_SHEET } from "./view";

const cells = (s: { grid: number[] }) => s.grid[0] * s.grid[1];

test("往里走：画面上更高、离相机更远；跳起来只改高低不改前后", () => {
  expect(footY(100)).toBeGreaterThan(footY(10));
  expect(zOf(100)).toBeLessThan(zOf(10));
  expect(footY(10, 50)).toBe(footY(10) + 50);
  expect(zOf(DEPTH)).toBeGreaterThan(0);
});

test("模型和站在同一纵深的精灵 y、z 对得上：物件挡住后面的人、被前面的人挡住", () => {
  const cart = modelAt("c", "cart", 500, 60);
  expect([cart.y, cart.z]).toEqual([footY(60), zOf(60)]);
  const g = newGame();
  const at = (d: number): Game => ({ ...g, player: { ...g.player, x: 500, d } });
  const z = (game: Game) => actorSprites(game).find((s) => s.id === "player")!.z!;
  // 车身占纵深 [36, 84]：前面的人 z 比车前脸大，后面的人比车后边小
  expect(z(at(30))).toBeGreaterThan(zOf(36));
  expect(z(at(90))).toBeLessThan(zOf(84));
});

test("人按纵深排前后，影子在人下面、贴着地；精灵都在前景层（z ≥ 0）", () => {
  const g0 = newGame();
  const g: Game = { ...g0, enemies: [newEnemy("near", "soldier", 300, 10, 0), newEnemy("far", "soldier", 300, 120, 1)] };
  const s = actorSprites(g);
  const z = (id: string) => s.find((x) => x.id === id)!.z!;
  expect(z("near")).toBeGreaterThan(z("far"));
  expect(z("near-shadow")).toBeLessThan(z("near"));
  expect(z("near-shadow")).toBeGreaterThan(z("far"));
  expect(s.every((x) => x.z! >= 0)).toBe(true);
  // 精灵中心按图集的脚底位置往上挪，脚踩在纵深那条线上
  const near = s.find((x) => x.id === "near")!;
  expect(near.y! - TROOP_SHEET.foot).toBe(footY(10));
  expect(s.find((x) => x.id === "near-shadow")!.shader).toBe("blob");
});

test("朝左时宽度是负的（镜像）；挨打闪白", () => {
  const g0 = newGame();
  const e = { ...newEnemy("e", "soldier", 300, 50, 0), face: -1 as const, flash: 0.1 };
  const s = actorSprites({ ...g0, enemies: [e] });
  const sprite = s.find((x) => x.id === "e")!;
  expect(sprite.w).toBe(-TROOP_SHEET.size[0]);
  expect(sprite.shader).toBe("flash");
  expect(s.find((x) => x.id === "player")!.w).toBe(PLAYER_SHEET.size[0]);
  expect(s.find((x) => x.id === "player")!.shader).toBe(undefined);
});

test("每个动作的帧都在图集里", () => {
  const g = newGame();
  const p = g.player;
  const poses = [
    { ...p }, { ...p, act: "walk" as const, t: 0.3 }, { ...p, act: "attack" as const, step: 3, t: ATTACKS[2].dur - 0.01 }, { ...p, act: "thrust" as const },
    { ...p, act: "jump" as const, vh: -1 }, { ...p, act: "plunge" as const, t: 1 }, { ...p, act: "land" as const }, { ...p, act: "hurt" as const }, { ...p, act: "dead" as const, t: 2 },
  ];
  for (const a of poses) {
    const f = playerFrame(a, 1.3);
    expect(f >= 0 && f < cells(PLAYER_SHEET)).toBe(true);
  }
  const acts = {
    soldier: ["idle", "walk", "windup", "attack", "recover", "hurt", "launched", "down", "getup", "dead"],
    archer: ["idle", "walk", "draw", "recover", "hurt", "launched", "down", "getup", "dead"],
    boss: ["idle", "walk", "windup", "attack", "recover", "chargeWindup", "charge", "hurt", "dead"],
  } as const;
  for (const kind of ["soldier", "archer", "boss"] as const) {
    const sheet = kind === "boss" ? BOSS_SHEET : TROOP_SHEET;
    for (const act of acts[kind]) {
      const f = enemyFrame({ ...newEnemy("e", kind, 0, 0, 0), act, t: 0.2 }, 0.7);
      expect(`${kind}.${act}:${f >= 0 && f < cells(sheet)}`).toBe(`${kind}.${act}:true`);
    }
  }
});

test("刀光和箭是特效精灵，在人前面一点；挨过打的小兵头上有血条，首领没有", () => {
  const g0 = newGame();
  const g: Game = {
    ...g0,
    fx: [{ id: "fx-1", name: "slash2", x: 0, d: 40, h: 50, face: -1, t: 0.05, dur: 0.14 }],
    arrows: [{ id: "arrow-1", x: 100, d: 40, h: 50, vx: -380 }],
    enemies: [{ ...newEnemy("s", "soldier", 0, 40, 0), hp: 20 }, { ...newEnemy("b", "boss", 0, 40, 1), hp: 20 }],
  };
  const fx = fxSprites(g);
  expect(fx.map((s) => s.id)).toEqual(["fx-1", "arrow-1"]);
  expect(fx.every((s) => s.z === zOf(40) + LAYER.fx)).toBe(true);
  expect(fx[0].w! < 0 && fx[1].w! < 0).toBe(true);
  expect(enemyBars(g).map((b) => [b.follow, b.value])).toEqual([["s", 0.5]]);
});

test("只放镜头附近的模型：地面、物件；旌旗在飘；镜头到了桥头才有桥", () => {
  const start = propModels(400);
  expect(start.some((m) => m.id.startsWith("ground-"))).toBe(true);
  expect(start.some((m) => m.id === "bridge")).toBe(false);
  expect(start.length).toBeLessThan(15);
  const banner = start.find((m) => m.model.endsWith("banner.glb"))!;
  expect(banner.animation?.clip).toBe("wave");
  const end = propModels(4500);
  expect(end.some((m) => m.id === "bridge")).toBe(true);
  // 每个物件总会在某个镜头位置出现，文件名都对
  const files = new Set(PROPS.flatMap((p) => propModels(p.x).map((m) => m.model)));
  for (const f of ["wall", "cart", "cart_fire", "banner", "barricade", "ground"]) expect(files.has(`asset:/@sanguo/${f}.glb`)).toBe(true);
});

test("场景：正交相机跟 2D 对齐、按深度混排；太阳投影子；暂停时间停住；声音预加载", () => {
  const s = sceneOf(newGame(), true, 3);
  expect(s.camera3d).toEqual({ match2d: true, interleave: true });
  expect(s.lights![0].shadows).toBe(true);
  expect(s.lights![0].kind).toBe("directional");
  expect([s.timeScale, s.level]).toEqual([0, 3]);
  expect(sceneOf(newGame(), false, 3).timeScale).toBe(1);
  expect(s.audio!.preload!.length).toBe(Object.keys(SOUNDS).length);
  // 背景层（z < 0）只有远景
  expect(s.sprites!.filter((x) => (x.z ?? 0) < 0).map((x) => x.id)).toEqual(["sky", "sun-disk", "hills-far", "hills-near"]);
});
