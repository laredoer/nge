// 3D 试验场的场景（见 docs/gameplay3d.md）。
import { expect, test } from "nsh/test";
import { LAB_HERO, LAB_LEVEL, labScene } from "./lab";

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
