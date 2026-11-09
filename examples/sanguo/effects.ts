// 黄昏的光和画面效果：低角度的太阳投影子，着火粮车的点光源、火焰和烟，bloom，远景（天、太阳、两层山）和背景层的模糊。
import type { Emitter3dNode, Light2dNode, LightNode, PostNode, SpriteNode } from "../../ts/nge";
import { footY, zOf } from "./belt";
import { PROPS, type Prop } from "./levels";

/** 自己写的着色器（WGSL）。 */
export const SHADERS: Record<string, string> = {
  // 黄昏的天：下面橙、中间粉、上面紫
  sky: `fn effect(e: Effect) -> vec4f {
    let y = e.local.y;
    let low = vec3f(1.0, 0.6, 0.3);
    let mid = vec3f(0.6, 0.28, 0.33);
    let top = vec3f(0.14, 0.09, 0.24);
    let c = select(mix(mid, top, (y - 0.35) / 0.65), mix(low, mid, y / 0.35), y < 0.35);
    return vec4f(c, 1.0);
}`,
  // 脚下的影子：中间深、往外渐淡的椭圆
  blob: `fn effect(e: Effect) -> vec4f {
    let p = (e.local - vec2f(0.5)) * 2.0;
    let a = 1.0 - smoothstep(0.35, 1.0, length(p));
    return vec4f(e.tint.rgb, e.tint.a * a);
}`,
  // 背景层的模糊：5×5 格取平均，params.x 是格子之间隔几个屏幕像素
  blur: `fn post(p: Post) -> vec4f {
    let step = p.params.x / p.size;
    var sum = vec4f(0.0);
    for (var i = -2; i <= 2; i++) {
        for (var j = -2; j <= 2; j++) {
            sum += sample(p.uv + vec2f(f32(i), f32(j)) * step);
        }
    }
    return sum / 25.0;
}`,
};

/** 火光、刀光往外晕；远景（天、山）稍微糊一点；四角压暗。 */
export const POST: PostNode[] = [
  { shader: "bloom", params: [0.72, 0.9, 5, 0] },
  { shader: "blur", params: [1.6, 0, 0, 0], layer: "back" },
  { shader: "vignette", params: [0.55, 0.4, 0, 0] },
];

/** 挨打闪白：混 85% 的白。 */
export const FLASH: Pick<SpriteNode, "shader" | "params"> = { shader: "flash", params: [1, 1, 1, 0.85] };

/** 太阳：从右前方低低地照过来，影子往左后方拉得很长。 */
export const SUN: LightNode = { id: "sun", kind: "directional", direction: [-1.5, -0.6, -0.4], color: "#ffbf86", intensity: 3.2, shadows: true };
export const AMBIENT = { color: "#8c6f99", brightness: 0.5 };
/** 2D 精灵（人、箭）受的光：暖暗的环境光，火边上亮。 */
export const SPRITE_AMBIENT = "#d2ae9e";

/** 着火的粮车。 */
export const FIRES: Prop[] = PROPS.filter((p) => p.kind === "fire");
/** 离镜头多远以内的火才点灯、冒火。 */
const NEAR = 900;

export function firesNear(cam: number): Prop[] {
  return FIRES.filter((f) => Math.abs(f.x - cam) < NEAR);
}

/** 火在车上的位置（带子坐标里车中间、麻袋顶上）换成 3D 坐标。 */
function fireAt(f: Prop, h: number): { x: number; y: number; z: number } {
  return { x: f.x - 4, y: footY(f.d, h), z: zOf(f.d) };
}

/** 着火粮车的点光源（照亮车、地面、旁边的东西）。亮度按距离平方衰减，按 80 远处约 1.5 配。 */
export function fireLights(cam: number): LightNode[] {
  return firesNear(cam).map((f, i) => ({ id: `fire-light-${i}`, kind: "point", ...fireAt(f, 60), color: "#ff8a3a", intensity: 1.5 * 80 * 80, range: 280 }));
}

/** 火焰（叠加混合，bloom 晕开）和往上飘的黑烟。 */
export function fireEmitters(cam: number): Emitter3dNode[] {
  return firesNear(cam).flatMap((f): Emitter3dNode[] => {
    const at = fireAt(f, 36);
    const key = `${f.x}-${f.d}`;
    return [
      {
        id: `flame-${key}`, ...at, rate: 70, lifetime: [0.4, 0.9], speed: [30, 70], direction: [0, 1, 0], spread: 25, gravity: [0, 50, 0],
        size: [16, 3], spin: [-180, 180], color: ["#ffd36aff", "#ff3a0a00"], blend: "add", max: 200,
      },
      {
        id: `smoke-${key}`, ...at, y: at.y + 30, rate: 8, lifetime: [1.6, 2.6], speed: [20, 40], direction: [-0.3, 1, 0], spread: 15, gravity: [-8, 10, 0],
        size: [14, 40], color: ["#2a2224a0", "#4a404400"], blend: "alpha", max: 60,
      },
    ];
  });
}

/** 精灵受的火光。 */
export function fireLights2d(cam: number): Light2dNode[] {
  return firesNear(cam).map((f) => ({ x: f.x, y: footY(f.d, 50), radius: 260, color: "#ff9a4a", intensity: 1.1 }));
}

/** 远景（z < 0 的背景层，画在 3D 后面）：天、太阳、两层山。天和太阳钉在屏幕上，山按视差慢慢挪。 */
export const BACKDROP: SpriteNode[] = [
  { id: "sky", x: 0, y: 260, w: 4000, h: 900, z: -300, parallax: 0, shader: "sky", unlit: true },
  { id: "sun-disk", x: 230, y: 70, w: 80, h: 80, z: -280, parallax: 0, color: "#ffd9a0", shader: "circle", unlit: true },
  { id: "hills-far", texture: "asset:/@sanguo/hills.png", x: 0, y: 70, w: 1024, h: 400, z: -200, parallax: [0.12, 0], repeat: "x", color: "#7a5070", unlit: true },
  { id: "hills-near", texture: "asset:/@sanguo/hills.png", x: 300, y: 30, w: 768, h: 300, z: -150, parallax: [0.3, 0], repeat: "x", color: "#40304a", unlit: true },
];
