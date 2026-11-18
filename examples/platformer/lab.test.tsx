// 3D 试验场的场景（见 docs/gameplay3d.md）。
import { expect, test } from "nsh/test";
import { LAB_FIREWORK, LAB_HERO, LAB_LEVEL, LAB_SHOCKWAVE, LAB_SMOKE, labAction, labScene, levelAfterLab } from "./lab";

const scene = labScene();
const model = (id: string) => scene.models!.find((m) => m.id === id)!;

test("一份单独的 3D 场景：有角色，没有 2D 的精灵和关卡", () => {
  expect(scene.level).toBe(LAB_LEVEL);
  expect(scene.sprites ?? []).toEqual([]);
  expect(scene.tilemaps ?? []).toEqual([]);
  expect(model(LAB_HERO).body).toEqual({ kind: "character", shape: "capsule", r: 12, h: 24, step: 20, snap: 4, friction: 0 });
});

test("斜坡是三角网格，台阶每级一个凸包，石头和门是凸包，平台是 kinematic", () => {
  expect([model("lab-ramps").body!.kind, model("lab-ramps").body!.shape]).toEqual(["fixed", "trimesh"]);
  expect(model("lab-steps").body!.shape).toBe("hulls");
  expect(model("lab-rock").body!.shape).toBe("convex");
  expect(model("lab-rock").scale).toBe(30);
  expect(model("lab-door").body!.shape).toBe("convex");
  expect(model("lab-platform").body!.kind).toBe("kinematic");
});

test("门是绕竖轴的铰链，限 ±100°，带阻力的马达；链子五节球窝吊着重球", () => {
  const joints = scene.joints3d!;
  const hinge = joints.find((j) => j.id === "lab-hinge")!;
  expect([hinge.kind, hinge.b, hinge.axis, hinge.limits, hinge.motor?.speed]).toEqual(["revolute", "lab-door", [0, 1, 0], [-100, 100], 0]);
  const chain = joints.filter((j) => j.id.startsWith("lab-chain-"));
  expect(chain.length).toBe(6);
  expect(chain.every((j) => j.kind === "ball")).toBe(true);
  expect(chain[0].a).toBe(undefined);
  expect(chain.map((j) => j.b)).toEqual(["lab-link-0", "lab-link-1", "lab-link-2", "lab-link-3", "lab-link-4", "lab-weight"]);
  // 每个关节两头都是场景里带 body 的模型
  for (const j of joints) for (const id of [j.a, j.b]) if (id !== undefined) expect(model(id).body === undefined).toBe(false);
});

test("地面是盒子：第一帧就有，不用等模型生成碰撞体", () => {
  expect(model("lab-floor").body).toEqual({ kind: "fixed", shape: "box", size: [1600, 20, 1600] });
  expect(model("lab-floor").y).toBe(-10);
});

test("试验场里只有 lab 动作管用：改键、换存档、语言、联网都不响应；lab 只在电脑上开关", () => {
  expect(labAction("lab", false, true)).toBe("toggle");
  expect(labAction("lab", true, true)).toBe("toggle");
  expect(labAction("lab", false, false)).toBe("ignore");
  for (const action of ["remap", "slot", "lang", "net"]) {
    expect(labAction(action, true, true)).toBe("ignore");
    expect(labAction(action, false, true)).toBe("pass");
  }
});

test("刚通关、还没换关就进试验场：出来时去下一关；最后一关之后回第一关", () => {
  expect(levelAfterLab(1, false, 3)).toBe(1);
  expect(levelAfterLab(1, true, 3)).toBe(2);
  expect(levelAfterLab(2, true, 3)).toBe(0);
});

test("画质：程序天空、雾、PCSS 软阴影、三段级联带过渡；点光源和聚光灯都投影子", () => {
  expect(scene.sky!.cube).toBe(undefined);
  expect(scene.sky!.sun).toBe(undefined);
  expect([scene.fog!.start, scene.fog!.end, scene.fog!.sky]).toEqual([700, 3000, 0.6]);
  expect(scene.fog!.height!.density! > 0).toBe(true);
  expect([scene.shadows!.filter, scene.shadows!.blend, scene.shadows!.maxLights]).toEqual(["pcss", 0.15, 2]);
  expect(scene.camera3d!.shadowCascades).toBe(3);
  const lights = scene.lights!;
  expect(lights.filter((l) => l.shadows).map((l) => l.kind)).toEqual(["directional", "point", "spot"]);
  expect(lights.find((l) => l.kind === "spot")!.shadowPriority).toBe(1);
  expect(scene.ambient!.brightness).toBe(0.1);
});

test("3D 粒子：烟花、冲击波、烟都在场景里，摆在地面上、互不重叠", () => {
  expect(scene.emitters3d!.map((e) => e.id)).toEqual(["lab-firework", "lab-shockwave", "lab-smoke"]);
  const spots = scene.emitters3d!.map((e) => [e.x!, e.z!]);
  for (const [i, a] of spots.entries()) for (const b of spots.slice(i + 1)) expect(Math.hypot(a[0] - b[0], a[1] - b[1])).toBeGreaterThan(150);
  for (const [x, z] of spots) expect(Math.max(Math.abs(x), Math.abs(z)) < 800).toBe(true);
});

test("烟花：往上打，死的时候在球面上沿法线炸开（子发射器），带阻尼和渐变", () => {
  expect([LAB_FIREWORK.rate! > 0, LAB_FIREWORK.facing, LAB_FIREWORK.direction ?? [0, 1, 0]]).toEqual([true, "velocity", [0, 1, 0]]);
  // 炸开时还在往上走或刚过最高点：寿命不超过 速度 / 重力
  expect(LAB_FIREWORK.lifetime![1] <= LAB_FIREWORK.speed![0] / -LAB_FIREWORK.gravity![1]).toBe(true);
  const [burst] = LAB_FIREWORK.subEmitters!;
  expect([burst.on, burst.count, burst.emitter.shape!.kind, burst.emitter.emitDirection]).toEqual(["death", 120, "sphere", "normal"]);
  expect(burst.emitter.drag! > 0 && burst.emitter.colorGradient!.length > 2).toBe(true);
  expect(burst.emitter.max! >= burst.count * LAB_FIREWORK.rate! * burst.emitter.lifetime![1]).toBe(true);
});

test("冲击波：水平铺在地面上方一点，从小扩到大、先亮后淡", () => {
  expect([LAB_SHOCKWAVE.facing, LAB_SHOCKWAVE.y, LAB_SHOCKWAVE.speed]).toEqual(["horizontal", 1, [0, 0]]);
  const size = LAB_SHOCKWAVE.sizeCurve!;
  expect(size[size.length - 1][1] > size[0][1] * 10).toBe(true);
  expect(LAB_SHOCKWAVE.alphaCurve![0][1]).toBe(0);
  expect(LAB_SHOCKWAVE.alphaCurve![LAB_SHOCKWAVE.alphaCurve!.length - 1][1]).toBe(0);
});

test("烟：软粒子、受光、湍流，在石头边上冒", () => {
  expect([LAB_SMOKE.soft! > 0, LAB_SMOKE.lit, LAB_SMOKE.noise!.strength! > 0, LAB_SMOKE.blend ?? "alpha"]).toEqual([true, true, true, "alpha"]);
  const rock = model("lab-rock");
  expect(Math.hypot(LAB_SMOKE.x! - rock.x!, LAB_SMOKE.z! - rock.z!) < 60).toBe(true);
});
