// 3D 试验场的场景（见 docs/gameplay3d.md）。
import { expect, test } from "nsh/test";
import { LAB_HERO, LAB_LEVEL, labAction, labScene, levelAfterLab } from "./lab";

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
