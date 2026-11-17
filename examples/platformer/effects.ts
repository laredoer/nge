// 平台跳跃的 2D 特效：金币扫过的亮光、终点上方的传送门、第三关的夜色（含回声）、被撞回去时闪白（见 docs/effects2d.md）；
// 亮处晕开、远景糊一点、夜里的石碑有凹凸、挡住角色的灯（见 docs/render2d-more.md）。
import type { AudioNode, LightingNode, PostNode, SpriteNode } from "../../ts/nge";

/** 自己写的着色器（WGSL）。 */
export const SHADERS: Record<string, string> = {
  // 一道亮光斜着扫过金币，每 params.x 秒一次；按世界 x 错开，不会所有金币一起闪
  shimmer: `fn effect(e: Effect) -> vec4f {
    let t = fract(e.time / e.params.x + e.world.x * 0.002);
    let band = 1.0 - smoothstep(0.0, 0.15, abs(e.local.x + e.local.y - t * 3.0 + 0.5));
    return vec4f(mix(e.color.rgb, vec3f(1.0), band * 0.8), e.color.a);
}`,
  // 慢慢转的星空：一格一格随机点星星，一闪一闪；params.x 是转速（弧度 / 秒）
  starfield: `fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(127.1, 311.7))) * 43758.5453);
}

fn effect(e: Effect) -> vec4f {
    let c = e.local - vec2f(0.5);
    let a = e.time * e.params.x;
    let r = vec2f(c.x * cos(a) - c.y * sin(a), c.x * sin(a) + c.y * cos(a)) * 12.0;
    let h = hash(floor(r));
    let d = length(fract(r) - vec2f(0.5));
    let star = step(0.8, h) * (1.0 - smoothstep(0.0, 0.25, d)) * (0.6 + 0.4 * sin(e.time * 3.0 + h * 40.0));
    let base = mix(vec3f(0.25, 0.1, 0.45), vec3f(0.04, 0.02, 0.12), length(c) * 2.0);
    return vec4f(base + vec3f(star), 1.0);
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

/** 每关都有的后处理：亮的地方（金币、终点的绿灯附近）往外晕开；只给背景层（远山、树）的轻微模糊。 */
export const POST: PostNode[] = [
  { shader: "bloom", params: [0.6, 0.9, 5, 0] },
  { shader: "blur", params: [1.5, 0, 0, 0], layer: "back" },
];

/** 夜里那关的石碑：带法线图（灯从哪边来哪边亮），挡住角色的灯（开了 shadows），走过去时影子往外甩。没有碰撞体。 */
export function stonesNear(x: number): SpriteNode[] {
  return [4, 9].map((tiles, i) => ({
    id: `stone-${i}`,
    x: x + tiles * 32,
    y: 24,
    w: 24,
    h: 48,
    z: 1,
    color: "#8a93a8",
    normal: "asset:/@platformer/stone_n.png",
    occluder: true,
  }));
}

/** 金币：亮光每 1.6 秒扫一次。 */
export const COIN_SHADER: Pick<SpriteNode, "shader" | "params"> = { shader: "shimmer", params: [1.6, 0, 0, 0] };

/** 被撞回起点后闪白的那一下：混 80% 的白，持续多久（毫秒）。 */
export const FLASH: Pick<SpriteNode, "shader" | "params"> = { shader: "flash", params: [1, 1, 1, 0.8] };
export const FLASH_MS = 250;

/** 夜里那关的音效带回声（音效分组加混响，见 docs/audio-more.md），别的关不加。 */
export function levelAudio(night: boolean): AudioNode {
  return night ? { buses: { sfx: { reverb: 0.35 } } } : {};
}

/** 传送门的半径，在终点上方多高。 */
export const PORTAL_R = 32;
const PORTAL_ABOVE = 110;

/** 终点上方的传送门：一圈紫边，圆形遮罩里是慢慢转的星空和一颗转着的方块星（转到角伸出圆外的地方被裁掉）。没有碰撞体。 */
export function portalOver(goal: SpriteNode): SpriteNode[] {
  const x = goal.x ?? 0;
  const y = (goal.y ?? 0) + PORTAL_ABOVE;
  const d = PORTAL_R * 2;
  return [
    { id: "portal-ring", x, y, w: d + 12, z: 3, color: "#b388ff", shader: "circle", unlit: true },
    { id: "portal-mask", x, y, w: d, z: 3, shader: "circle", mask: true },
    { id: "portal-sky", x, y, w: d, z: 3.1, shader: "starfield", params: [0.3, 0, 0, 0], masked: "inside", unlit: true },
    { id: "portal-star", x, y, w: d * 0.8, rotation: 45, spin: 90, z: 3.2, color: "#fff6c2a0", masked: "inside", unlit: true },
  ];
}

/** 夜里：环境光暗，角色带一盏暖色的灯，终点和传送门各一盏绿、紫的；四角压暗。 */
export function night(goal: SpriteNode): { lighting: LightingNode; post: PostNode[] } {
  const x = goal.x ?? 0;
  const y = goal.y ?? 0;
  return {
    lighting: {
      ambient: "#4a5578",
      lights: [
        // 角色的灯低一点（石碑的凹凸明显），会被石碑挡住
        { follow: "player", y: 10, radius: 260, color: "#ffd9a0", intensity: 1.1, height: 40, shadows: true },
        { x, y, radius: 140, color: "#6bcb77", intensity: 0.8 },
        { x, y: y + PORTAL_ABOVE, radius: 150, color: "#b388ff", intensity: 0.9 },
      ],
    },
    post: [{ shader: "vignette", params: [0.6, 0.45, 0, 0] }],
  };
}
