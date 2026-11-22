// 风语群岛 3D 开放世界场景声明：动态昼夜天空、PCSS 级联软阴影、地形 trimesh、物理遗迹门、浮空石台与 3D 粒子。
import type { BarNode, BubbleNode, Decal3dNode, Emitter3dNode, Joint3dNode, LightNode, ModelNode, PostNode, Scene, SkyNode, FogNode } from "../../ts/nge";
import world from "./world.json";

export type TimeOfDay = 0 | 1 | 2 | 3; // 0 晨曦 · 1 晴空 · 2 暮霞 · 3 星夜
export type ToneMapOption = "aces" | "reinhard" | "none";
export type ShadingPreset = "stylized" | "toon" | "raw";
export type Fx3dPreset = "aura" | "holo" | "off";

export interface QualitySettings {
  tonemap: ToneMapOption;
  ssao: boolean;
  fxaa: boolean;
  bloom: boolean;
  shading: ShadingPreset;
  fx3d: Fx3dPreset;
  navDebug: boolean;
  perf: boolean;
}

export const DEFAULT_QUALITY: QualitySettings = {
  tonemap: "aces",
  ssao: true,
  fxaa: true,
  bloom: true,
  shading: "stylized",
  fx3d: "aura",
  navDebug: true,
  perf: true,
};

/** 自定义 3D 材质着色器示例（WGSL 片段，通过 scene.shaders3d 热注册）。 */
const CUSTOM_SHADERS_3D: Record<string, string> = {
  aura: `
fn shade3d(s: Surface) -> vec4f {
    let rim = pow(clamp(1.0 - max(dot(s.normal, s.view), 0.0), 0.0, 1.0), 2.4);
    let wave = 0.55 + 0.45 * sin(s.world.y * 0.22 - s.time * 4.5);
    let glow = s.params.rgb * rim * wave * s.params.w;
    return vec4f(s.color.rgb + glow, s.color.a);
}
`,
  holo: `
fn shade3d(s: Surface) -> vec4f {
    let rim = pow(clamp(1.0 - max(dot(s.normal, s.view), 0.0), 0.0, 1.0), 1.8);
    let scan = 0.72 + 0.28 * sin(s.world.y * 1.4 - s.time * 8.0);
    let base = mix(s.color.rgb * 0.35, s.params.rgb * (0.65 + rim * 1.8), 0.75) * scan;
    return vec4f(base * s.params.w, s.color.a);
}
`,
};

export interface ShrineInfo {
  id: string;
  name: string;
  region: string;
  x: number;
  y: number;
  z: number;
  lore: string;
}

export interface RelicInfo {
  id: string;
  x: number;
  y: number;
  z: number;
}

export const SHRINES: ShrineInfo[] = world.shrines;
export const RELICS: RelicInfo[] = world.relics;
export const CAMP = world.camp;

export const SOUNDS = {
  shrine: "asset:/@whispering-isles/shrine.wav",
  relic: "asset:/@whispering-isles/relic.wav",
  time: "asset:/@whispering-isles/time.wav",
};

interface Atmosphere {
  label: string;
  bg: string;
  sky: SkyNode;
  fog: FogNode;
  sunDir: [number, number, number];
  sunColor: string;
  sunIntensity: number;
  ambient: number;
  nightGlow: number;
}

export const ATMOSPHERES: Record<TimeOfDay, Atmosphere> = {
  0: {
    label: "晨曦",
    bg: "#8db8dc",
    sky: { top: "#3868b0", horizon: "#f5c79e", ground: "#4c5a46", sunSize: 2.6 },
    fog: { start: 850, end: 3600, color: "#ecccb0", sky: 0.7, max: 0.85, height: { base: -10, falloff: 0.015, density: 0.0007 } },
    sunDir: [-1.4, -0.85, -1.0],
    sunColor: "#ffd4a8",
    sunIntensity: 2.8,
    ambient: 0.16,
    nightGlow: 0.9,
  },
  1: {
    label: "晴空",
    bg: "#6caae6",
    sky: { top: "#2464c2", horizon: "#cde5fa", ground: "#4b6244", sunSize: 2.0 },
    fog: { start: 1000, end: 4200, color: "#cde5fa", sky: 0.65, max: 0.8, height: { base: -20, falloff: 0.018, density: 0.0005 } },
    sunDir: [-0.8, -2.0, -0.9],
    sunColor: "#fff8eb",
    sunIntensity: 3.2,
    ambient: 0.22,
    nightGlow: 0.6,
  },
  2: {
    label: "暮霞",
    bg: "#c7745c",
    sky: { top: "#3b2c68", horizon: "#ff8c5a", ground: "#3b2a28", sunSize: 3.2 },
    fog: { start: 750, end: 3400, color: "#e68e6e", sky: 0.75, max: 0.88, height: { base: -10, falloff: 0.016, density: 0.0009 } },
    sunDir: [1.5, -0.55, -0.9],
    sunColor: "#ff9a5c",
    sunIntensity: 2.9,
    ambient: 0.14,
    nightGlow: 1.35,
  },
  3: {
    label: "星夜",
    bg: "#0b1428",
    sky: { top: "#060b1c", horizon: "#1a2f52", ground: "#101824", sun: [0.8, -1.2, 0.9], sunColor: "#c8e0ff", sunSize: 2.4, diffuse: 0.35, specular: 0.5 },
    fog: { start: 650, end: 3000, color: "#152542", sky: 0.65, max: 0.9, height: { base: -10, falloff: 0.015, density: 0.001 } },
    sunDir: [0.8, -1.2, 0.9],
    sunColor: "#8cb8ff",
    sunIntensity: 1.15,
    ambient: 0.12,
    nightGlow: 2.0,
  },
};

export function regionOf(x: number, z: number): string {
  if (Math.hypot(x, z + 760) < 330) return "星穹峰顶";
  if (Math.hypot(x + 580, z + 260) < 360) return "翡翠松林";
  if (Math.hypot(x - 540, z + 320) < 280) return "风蚀石林遗迹";
  if (Math.hypot(x + 440, z - 440) < 320) return "晶蓝湖湾";
  if (Math.hypot(x, z - 220) < 240) return "晨曦营地";
  return "风语群岛";
}

export function worldScene(
  awakened: Set<string>,
  collected: Set<string>,
  tod: TimeOfDay,
  round: number,
  quality: QualitySettings = DEFAULT_QUALITY,
  beacon: [number, number, number] | null = null,
): Scene {
  const atmo = ATMOSPHERES[tod];
  const solidStyle = quality.shading === "toon" ? "toon" : "pbr";
  const waterStyle = quality.shading === "raw" ? "pbr" : "water";
  const foliageStyle = quality.shading === "raw" ? "pbr" : "foliage";

  const models: ModelNode[] = [
    // 起伏群岛地形（trimesh 碰撞体）与碧蓝海面
    {
      id: "terrain",
      model: "asset:/@whispering-isles/terrain.glb",
      x: 0,
      y: 0,
      z: 0,
      style: solidStyle,
      body: { kind: "fixed", shape: "trimesh", friction: 0.7 },
    },
    {
      id: "water",
      model: "asset:/@whispering-isles/water.glb",
      x: 0,
      y: -16,
      z: 0,
      style: waterStyle,
    },
    // 玩家探索者（带 idle / walk 动画 + 程序化骨骼注视风灵伙伴 + 可实时切换的自定义 3D 着色器）与曲面滑翔伞
    {
      id: "explorer",
      model: "asset:/@whispering-isles/explorer.glb",
      x: world.spawn.x,
      y: world.spawn.y,
      z: world.spawn.z,
      animation: { clip: "idle", loop: true },
      lookAt: { bone: "body", target: "camp-sentinel", maxAngle: 65, weight: 0.95, speed: 8 },
      body: { kind: "character", shape: "capsule", r: 12, h: 24, slope: 48, step: 22, snap: 10 },
      ...(quality.fx3d !== "off"
        ? { shader: quality.fx3d, params: quality.fx3d === "aura" ? [0.35, 1.45, 1.85, 1.35] : [0.25, 0.95, 1.45, 1.25] }
        : {}),
    },
    // 3D 骨骼挂点实战：把发光风杖挂在探索者右手骨骼 armR 上，随行走/待机挥臂同步运动并拖出风灵光带
    {
      id: "wind-staff",
      model: "asset:/@whispering-isles/staff.glb",
      socket: { target: "explorer", bone: "armR" },
      x: 0,
      y: -8.5,
      z: -2.0,
      rotation: [-22, 0, -6],
      trail: { lifetime: 0.35, width: [4.5, 0], color: ["#5ef2ffff", "#5ef2ff00"], minDistance: 3, blend: "add" },
      ...(quality.fx3d !== "off"
        ? { shader: quality.fx3d, params: quality.fx3d === "aura" ? [0.45, 1.6, 2.0, 1.5] : [0.3, 1.0, 1.6, 1.3] }
        : {}),
    },
    // 3D 导航寻路实战 1：平时跟随探索者，左键点击地面后自动沿 3D NavMesh 飞往标点侦察
    {
      id: "wind-wisp",
      model: "asset:/@whispering-isles/wisp.glb",
      x: world.spawn.x - 42,
      y: world.spawn.y + 18,
      z: world.spawn.z + 24,
      animation: { clip: "float", loop: true },
      nav: beacon
        ? { target: beacon, speed: 195, arrive: 18, hover: 24, turnSpeed: 10 }
        : { follow: "explorer", speed: 155, minDistance: 46, hover: 24, turnSpeed: 8 },
      trail: { lifetime: 0.45, width: [5.5, 0], color: ["#48f5ffff", "#48f5ff00"], minDistance: 4, blend: "add" },
      ...(quality.fx3d !== "off"
        ? { shader: quality.fx3d, params: [0.35, 1.5, 1.9, 1.35] }
        : {}),
    },
    // 3D 导航寻路实战 2：营地与瞭望坛之间循环巡逻的巡林守卫灵
    {
      id: "camp-sentinel",
      model: "asset:/@whispering-isles/wisp.glb",
      x: -55,
      y: world.spawn.y + 18,
      z: 175,
      scale: 1.25,
      animation: { clip: "float", loop: true },
      lookAt: { bone: "wisp", target: "explorer", maxAngle: 75, weight: 1.0, speed: 10 },
      nav: {
        patrol: [
          [-65, 28, 185],
          [90, 28, 175],
          [155, 28, 75],
          [-25, 28, 65],
        ],
        speed: 88,
        hover: 26,
        wait: 1.0,
      },
      trail: { lifetime: 0.4, width: [5.0, 0], color: ["#ffd859ff", "#ffd85900"], minDistance: 4, blend: "add" },
      ...(quality.fx3d !== "off"
        ? { shader: quality.fx3d, params: [1.55, 1.15, 0.3, 1.3] }
        : {}),
    },
    {
      id: "glider",
      model: "asset:/@whispering-isles/glider.glb",
      x: 0,
      y: -600,
      z: 0,
    },
    // 营地篝火
    {
      id: "campfire",
      model: "asset:/@whispering-isles/campfire.glb",
      x: CAMP.x,
      y: CAMP.y,
      z: CAMP.z,
    },
  ];

  // 5 座古代圣坛（自带水晶悬浮旋转动画 float）
  for (const s of SHRINES) {
    const awake = awakened.has(s.id);
    models.push({
      id: s.id,
      model: awake ? "asset:/@whispering-isles/shrine_awake.glb" : "asset:/@whispering-isles/shrine_dormant.glb",
      x: s.x,
      y: s.y - 2,
      z: s.z,
      animation: { clip: "float", loop: true },
      body: { kind: "fixed", shape: "box", size: [60, 56, 60] },
    });
  }

  // 未收集的风之微光（旋转发光晶簇 + 金色能量光晕自定义 3D 着色器）
  for (const r of RELICS) {
    if (collected.has(r.id)) continue;
    models.push({
      id: r.id,
      model: "asset:/@whispering-isles/relic.glb",
      x: r.x,
      y: r.y,
      z: r.z,
      spin: 95,
      ...(quality.fx3d !== "off"
        ? { shader: quality.fx3d, params: quality.fx3d === "aura" ? [1.6, 1.15, 0.3, 1.25] : [0.25, 0.95, 1.45, 1.2] }
        : {}),
    });
  }

  // 多种有机树木：松树、阔叶橡树与樱粉灵树（开启风吹摆动 + 自动 GPU Instancing 合批）
  for (const t of world.trees) {
    models.push({
      id: t.id,
      model: `asset:/@whispering-isles/${t.kind}.glb`,
      x: t.x,
      y: t.y,
      z: t.z,
      scale: t.scale,
      rotation: [0, t.rot, 0],
      style: foliageStyle,
      body: { kind: "fixed", shape: "capsule", r: 9 * t.scale, h: 48 * t.scale },
    });
  }

  // 草丛与野花点缀（风吹摆动 + 自动 GPU Instancing 合批）
  for (const f of world.flowers) {
    models.push({
      id: f.id,
      model: "asset:/@whispering-isles/flowers.glb",
      x: f.x,
      y: f.y,
      z: f.z,
      scale: f.scale,
      rotation: [0, f.rot, 0],
      style: foliageStyle,
    });
  }

  // 旅途暖光路灯柱
  for (const l of world.lanterns) {
    models.push({
      id: l.id,
      model: "asset:/@whispering-isles/lantern.glb",
      x: l.x,
      y: l.y,
      z: l.z,
      rotation: [0, l.rot, 0],
    });
  }

  // 苔藓巨石
  for (const b of world.boulders) {
    models.push({
      id: b.id,
      model: "asset:/@whispering-isles/boulder.glb",
      x: b.x,
      y: b.y,
      z: b.z,
      scale: b.scale,
      rotation: [0, b.rot, 0],
      body: { kind: "fixed", shape: "convex" },
    });
  }

  // 风蚀遗迹：拱门、石柱、可推开的 3D 铰链木门与物理补给箱
  for (const a of world.ruins.arches) {
    models.push({
      id: a.id,
      model: "asset:/@whispering-isles/ruin_arch.glb",
      x: a.x,
      y: a.y,
      z: a.z,
      rotation: [0, a.rot, 0],
    });
  }
  for (const p of world.ruins.pillars) {
    models.push({
      id: p.id,
      model: "asset:/@whispering-isles/ruin_pillar.glb",
      x: p.x,
      y: p.y,
      z: p.z,
      body: { kind: "fixed", shape: "box", size: [34, 102, 34] },
    });
  }
  const gate = world.ruins.gate;
  models.push({
    id: gate.id,
    model: "asset:/@whispering-isles/ruin_gate.glb",
    x: gate.x,
    y: gate.y,
    z: gate.z,
    body: { kind: "dynamic", shape: "box", size: [82, 114, 10], density: 0.45 },
  });
  for (const c of world.ruins.crates) {
    models.push({
      id: c.id,
      model: "asset:/@whispering-isles/crate.glb",
      x: c.x,
      y: c.y,
      z: c.z,
      body: { kind: "dynamic", shape: "box", size: [36, 36, 36], density: 0.5 },
    });
  }

  // 2 块通往星穹峰顶的浮空移动石台（Rust 驱动位置，角色可站上去搭乘）
  for (const pl of world.platforms) {
    models.push({
      id: pl.id,
      model: "asset:/@whispering-isles/platform.glb",
      x: pl.x,
      y: pl.y,
      z: pl.z,
      body: { kind: "kinematic", shape: "box", size: [128, 20, 128] },
    });
  }

  // 遗迹铰链门 3D 关节（可推开 ±105°）
  const joints3d: Joint3dNode[] = [
    {
      id: "ruin-gate-hinge",
      b: gate.id,
      kind: "revolute",
      anchorB: [gate.x, gate.y, gate.z],
      axis: [0, 1, 0],
      limits: [-105, 105],
    },
  ];

  // 光源：太阳/月亮方向光（级联软阴影）+ 篝火暖光 + 圣坛光柱点光源
  const lights: LightNode[] = [
    { id: "sun", kind: "directional", direction: atmo.sunDir, color: atmo.sunColor, intensity: atmo.sunIntensity, shadows: true },
    { id: "camp-light", kind: "point", x: CAMP.x, y: CAMP.y + 38, z: CAMP.z, color: "#ff9b4a", intensity: 52000 * atmo.nightGlow, range: 420 },
  ];
  for (const s of SHRINES) {
    const awake = awakened.has(s.id);
    lights.push({
      id: `light-${s.id}`,
      kind: "point",
      x: s.x,
      y: s.y + 84,
      z: s.z,
      color: awake ? "#45e8ff" : "#ffca58",
      intensity: (awake ? 58000 : 34000) * atmo.nightGlow,
      range: 460,
    });
  }

  // 3D 粒子系统：篝火火焰与火星、翡翠松林萤火虫、圣坛冲天光柱
  const emitters3d: Emitter3dNode[] = [
    {
      id: "camp-fire",
      x: CAMP.x,
      y: CAMP.y + 10,
      z: CAMP.z,
      rate: 45,
      lifetime: [0.45, 0.9],
      speed: [22, 55],
      direction: [0, 1, 0],
      spread: 24,
      gravity: [0, 28, 0],
      size: [14, 2],
      color: ["#ffdf73ff", "#ff451000"],
      blend: "add",
      max: 140,
    },
    {
      id: "forest-fireflies",
      x: -580,
      y: 75,
      z: -260,
      rate: 16,
      lifetime: [3.2, 5.5],
      speed: [8, 20],
      shape: { kind: "box", size: [520, 90, 480] },
      emitDirection: "random",
      noise: { strength: 22, frequency: 0.012, scroll: 0.2 },
      sizeCurve: [[0, 2], [0.5, 6], [1, 1]],
      color: ["#c8ff75ff", "#75ff9e00"],
      blend: "add",
      max: 160,
    },
  ];

  for (const s of SHRINES) {
    const awake = awakened.has(s.id);
    emitters3d.push({
      id: `beacon-${s.id}`,
      x: s.x,
      y: s.y + 72,
      z: s.z,
      rate: awake ? 28 : 16,
      lifetime: [2.0, 3.4],
      speed: [75, 130],
      direction: [0, 1, 0],
      spread: 8,
      size: [awake ? 10 : 7, 2],
      color: awake ? ["#45f0ffff", "#45f0ff00"] : ["#ffd86bff", "#ff9e2c00"],
      blend: "add",
      max: 120,
    });
  }

  if (quality.shading === "toon") {
    for (const m of models) {
      if (!m.style) m.style = "toon";
    }
  }

  const post: PostNode[] = [];
  if (quality.bloom) post.push({ shader: "bloom", params: [0.88, 0.45, 3, 0] });
  if (quality.fxaa) post.push({ shader: "fxaa" });

  return {
    level: round,
    background: atmo.bg,
    perf: quality.perf,
    camera3d: { x: world.spawn.x, y: world.spawn.y + 180, z: world.spawn.z + 360, target: [world.spawn.x, world.spawn.y + 24, world.spawn.z], fov: 56, shadowCascades: 2, shadowDistance: 1500 },
    lights,
    ambient: { brightness: atmo.ambient },
    sky: atmo.sky,
    fog: atmo.fog,
    shadows: { filter: "pcf", size: 1024, blend: 0.1, maxLights: 0 },
    tonemap: quality.tonemap === "none" ? { mode: "none" } : { mode: quality.tonemap, exposure: 1.05, saturation: 1.08 },
    ssao: { enabled: quality.ssao, intensity: 0.55, radius: 0.02 },
    shaders3d: CUSTOM_SHADERS_3D,
    navmesh3d: { surface: "terrain", maxSlope: 48, minY: -8, agentRadius: 14, debug: quality.navDebug },
    pick3d: { exclude: "explorer", maxDistance: 3500 },
    post,
    physics3d: { gravity: [0, -981, 0] },
    audio: { preload: Object.values(SOUNDS) },
    // 3D 世界 UI 实战：头顶跟随 3D 模型的能量条与打字机对话气泡
    bars: [
      {
        id: "sentinel-bar",
        follow: "camp-sentinel",
        lift: 20,
        value: 0.88,
        w: 44,
        h: 5,
        color: "#fbbf24",
        back: "#000000a8",
      } satisfies BarNode,
    ],
    bubbles: [
      {
        id: "wisp-talk",
        follow: "wind-wisp",
        lift: 18,
        text: awakened.size === 0 ? "去点亮前方的古代圣坛吧！" : `已唤醒 ${awakened.size}/${SHRINES.length} 座圣坛 ✨`,
        size: 13,
        color: "#e0faff",
        back: "#082436c8",
        padding: 6,
        typing: 16,
      } satisfies BubbleNode,
    ],
    // 3D 贴地光环与法阵实战：探索者脚底风灵光环 + 5 座圣坛脚下旋转古代符文法阵 + 浮空风灵脚底接触影 + 点击地面扩散波纹标点
    decals3d: [
      { id: "hero-ring", kind: "ring", follow: "explorer", size: 46, color: "#48f5ffaa", spin: 28, pulse: 0.08, lift: 2.5 } satisfies Decal3dNode,
      ...(beacon
        ? [{ id: "pick-beacon", kind: "pulse", x: beacon[0], y: beacon[1], z: beacon[2], size: 58, color: "#ffe45eee", spin: 42, pulse: 0.16, lift: 3.0 } satisfies Decal3dNode]
        : []),
      { id: "wisp-shadow", kind: "shadow", follow: "wind-wisp", size: 28, lift: 2.0 } satisfies Decal3dNode,
      { id: "sentinel-shadow", kind: "shadow", follow: "camp-sentinel", size: 34, lift: 2.0 } satisfies Decal3dNode,
      ...SHRINES.map((s): Decal3dNode => {
        const awake = awakened.has(s.id);
        return {
          id: `rune-${s.id}`,
          kind: "rune",
          x: s.x,
          y: s.y,
          z: s.z,
          size: awake ? 148 : 118,
          color: awake ? "#38f2ffdd" : "#ffca58bb",
          spin: awake ? 26 : 12,
          pulse: awake ? 0.06 : 0.03,
          lift: 3.0,
        };
      }),
    ],
    models,
    joints3d,
    emitters3d,
  };
}
