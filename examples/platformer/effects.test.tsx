// 平台跳跃的 2D 特效（见 docs/effects2d.md）：传送门的遮罩、第三关的夜色。着色器能不能编过在 Rust 那边测（src/lib.rs）。
import { expect, test } from "nsh/test";
import { levelAudio, night, PORTAL_R, portalOver, POST, SHADERS, stonesNear } from "./effects";
import { LEVELS } from "./levels";

test("传送门在终点正上方：一个圆形遮罩，星空和星星只画在里面，边框不受遮罩", () => {
  for (const { built } of LEVELS) {
    const portal = portalOver(built.goal);
    const masks = portal.filter((s) => s.mask);
    expect(masks.length).toBe(1);
    expect(masks[0].shader).toBe("circle");
    expect(masks[0].w).toBe(PORTAL_R * 2);
    expect(portal.filter((s) => s.masked === "inside").map((s) => s.id)).toEqual(["portal-sky", "portal-star"]);
    expect(portal.find((s) => s.id === "portal-ring")?.masked).toBe(undefined);
    for (const s of portal) {
      expect(s.x).toBe(built.goal.x);
      expect(s.y! - PORTAL_R).toBeGreaterThan(built.goal.y! + built.goal.h! / 2);
      expect(s.body).toBe(undefined);
      // 遮罩只管同一层（z ≥ 0）
      expect(s.z!).toBeGreaterThan(0);
    }
    // 着色器都在库里（circle 是内置的）
    for (const s of portal.filter((s) => s.shader && s.shader !== "circle")) expect(SHADERS[s.shader!] === undefined).toBe(false);
  }
});

test("只有第三关是夜里：角色带灯，终点和传送门各一盏，四角压暗", () => {
  expect(LEVELS.map((l) => l.night === true)).toEqual([false, false, true]);
  const goal = LEVELS[2].built.goal;
  const { lighting, post } = night(goal);
  const lights = lighting.lights!;
  expect(lights[0].follow).toBe("player");
  expect(lights.slice(1).map((l) => [l.x, l.y! > goal.y!])).toEqual([[goal.x, false], [goal.x, true]]);
  expect(post.map((p) => p.shader)).toEqual(["vignette"]);
  expect(lights[0].shadows).toBe(true);
});

test("每关都有 bloom 和背景层的模糊；夜里的石碑带法线图、挡光、没有碰撞体", () => {
  expect(POST.map((p) => [p.shader, p.layer ?? "screen"])).toEqual([["bloom", "screen"], ["blur", "back"]]);
  expect(SHADERS.blur === undefined).toBe(false);
  const stones = stonesNear(96);
  expect(stones.length).toBe(2);
  for (const s of stones) {
    expect(s.normal).toBe("asset:/@platformer/stone_n.png");
    expect(s.occluder).toBe(true);
    expect(s.body).toBe(undefined);
    // 底边贴着地面（y = 0）
    expect(s.y! - s.h! / 2).toBe(0);
  }
});

test("夜里那关的音效有回声，别的关没有", () => {
  expect(levelAudio(true).buses?.sfx?.reverb).toBeGreaterThan(0);
  expect(levelAudio(true).buses?.music).toBe(undefined);
  expect(levelAudio(false)).toEqual({});
});
