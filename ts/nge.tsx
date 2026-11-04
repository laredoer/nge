// nge 的 TSX 绑定：<GameView> 组件和场景类型（和 crates/nge-scene 的协议一一对应）。
import { nativeComponent, type GestureResponderEvent, type NativeComponentProps, type YNode } from "nsh";

/** 一个精灵。坐标是世界坐标（point，y 朝上，原点在视口中心），角度用度（逆时针）。 */
export interface SpriteNode {
  /** 按它做增量同步；同一个场景里不能重复。 */
  id: string;
  x?: number;
  y?: number;
  /** 绘制顺序，大的在上面。 */
  z?: number;
  rotation?: number;
  scale?: number;
  /** 不填按纹理像素大小；只填一个时按正方形。 */
  w?: number;
  h?: number;
  /** "#rrggbb" / "#rrggbbaa"，乘到纹理上。 */
  color?: string;
  /** Rust 侧 `ScenePlugin` 注册的纹理名，或 `asset:/` 路径；不填是纯色矩形。 */
  texture?: string;
  /** `texture` 是 `asset:/` 路径时，按 [列, 行] 切成图集。 */
  grid?: [number, number];
  /** 图集格子序号。 */
  frame?: number;
  /** 每秒转多少度。 */
  spin?: number;
  /** 可点击：按下时发 tap 事件。 */
  tap?: boolean;
}

export type Anchor = "top-left" | "top" | "top-right" | "left" | "center" | "right" | "bottom-left" | "bottom" | "bottom-right";

/** 一段文字。默认在 2D 世界坐标里，跟着相机走。 */
export interface TextNode {
  id: string;
  text: string;
  x?: number;
  y?: number;
  /** 字号（point），默认 16。 */
  size?: number;
  color?: string;
  /** 哪一点对准 (x, y)，默认 "center"。 */
  anchor?: Anchor;
  /** 字体的 `asset:/` 路径；不填用系统字体。 */
  font?: string;
  /** x / y 是屏幕坐标（左上角原点，y 朝下），不跟相机走。 */
  screen?: boolean;
  /** 超过这个宽度自动换行。 */
  maxWidth?: number;
  /** 跟着这个模型（id）投影到屏幕的位置走，x / y 变成屏幕上的偏移（y 朝上）。 */
  follow?: string;
  /** 跟随模型时往上抬的 3D 距离。 */
  lift?: number;
}

/** 粒子参数。范围写成 [最小, 最大]，角度用度（0 朝右、逆时针）。 */
export interface Particles {
  x?: number;
  y?: number;
  z?: number;
  /** 寿命（秒），默认 [1, 1]。 */
  lifetime?: [number, number];
  /** 初速度（point / 秒），默认 [50, 100]。 */
  speed?: [number, number];
  /** 发射方向，默认 [0, 360]。 */
  angle?: [number, number];
  /** 加速度 [x, y]（y 朝上）。 */
  gravity?: [number, number];
  /** [出生, 死亡] 时的大小，默认 [8, 0]。 */
  size?: [number, number];
  /** [出生, 死亡] 时的颜色；不填是白色淡出。 */
  color?: [string, string];
  /** 注册过的纹理名或 `asset:/` 路径；不填是方块。 */
  texture?: string;
  /** 同时存活的上限，默认 10000。 */
  max?: number;
}

/** 一直发射的粒子发射器。 */
export interface EmitterNode extends Particles {
  id: string;
  /** 每秒发射多少个，默认 0（不发射）。 */
  rate?: number;
}

/** 一个 glTF 模型。3D 坐标右手系、y 朝上。 */
export interface ModelNode {
  id: string;
  /** `.glb` / `.gltf` 的 `asset:/` 路径。 */
  model: string;
  x?: number;
  y?: number;
  z?: number;
  /** [x, y, z]（度），按 Y、X、Z 的顺序转。 */
  rotation?: [number, number, number];
  scale?: number;
  /** 绕 y 轴每秒转多少度。 */
  spin?: number;
  /** 可点击：点中时发 tap 事件（带 3D 命中点）。 */
  tap?: boolean;
}

/** 3D 相机：在 (x, y, z) 看向 target。不填时在 (0, 2, 6) 看向原点。 */
export interface Camera3dNode {
  x?: number;
  y?: number;
  z?: number;
  target?: [number, number, number];
  /** 垂直视角（度），默认 60。 */
  fov?: number;
}

export interface LightNode {
  id: string;
  kind: "directional" | "point";
  /** 方向光照射的方向，默认朝下。 */
  direction?: [number, number, number];
  /** 点光源的位置。 */
  x?: number;
  y?: number;
  z?: number;
  color?: string;
  /** 方向光的照度 / 点光源的强度。 */
  intensity?: number;
  /** 点光源的照射范围。 */
  range?: number;
}

export interface Scene {
  /** 清屏色 "#rrggbb"。 */
  background?: string;
  camera?: { x?: number; y?: number; zoom?: number };
  sprites?: SpriteNode[];
  texts?: TextNode[];
  emitters?: EmitterNode[];
  models?: ModelNode[];
  camera3d?: Camera3dNode;
  lights?: LightNode[];
  ambient?: { color?: string; brightness?: number };
}

/** 点中了可点击的精灵或模型。x / y 是指针的 2D 世界坐标；点中模型时 point 是 3D 命中点。 */
export interface Tap {
  id: string;
  x: number;
  y: number;
  point?: [number, number, number];
}

/** `burst` 的参数：炸一团粒子，死完自动删除。 */
export interface Burst extends Particles {
  /** 发射多少个，默认 100。 */
  count?: number;
}

/** `floatText` 的参数：在 2D 世界坐标 (x, y) 飘出一行字，往上飘、渐隐。 */
export interface FloatText {
  x: number;
  y: number;
  text: string;
  /** 默认 24。 */
  size?: number;
  color?: string;
  font?: string;
  /** 秒，默认 1。 */
  duration?: number;
  /** 每秒往上飘多少 point，默认 60。 */
  rise?: number;
}

/** 炸一团粒子。`game` 是 `<GameView ref>` 拿到的节点。 */
export function burst(game: YNode | undefined, args: Burst) {
  game?.command("burst", args);
}

/** 飘一行字。`game` 是 `<GameView ref>` 拿到的节点。 */
export function floatText(game: YNode | undefined, args: FloatText) {
  game?.command("floatText", args);
}

export interface FrameStats {
  fps: number;
  worst_ms: number;
}

export interface EngineError {
  message: string;
}

interface BuiltinProps {
  scene: Scene;
}

interface BuiltinEvents {
  tap: Tap;
  fps: FrameStats;
  error: EngineError;
}

type TouchPhase = "start" | "move" | "end" | "cancel";

export type GameViewProps<P, E> = NativeComponentProps<Partial<BuiltinProps> & P, BuiltinEvents & E>;

/**
 * 生成游戏视图组件。`props` / `events` 是游戏自己的通道名（Rust 侧 `add_prop` / `add_state` / `add_event`），
 * 类型由泛型给出；内置 prop `scene`、事件 `tap` / `fps` / `error`，触摸自动转成引擎的 `touch` command。
 */
export function gameView<P extends object = {}, E extends object = {}>(
  spec: { props?: readonly (keyof P & string)[]; events?: readonly (keyof E & string)[] } = {},
) {
  const Native = nativeComponent<BuiltinProps & P, BuiltinEvents & E>("nge.game", {
    props: [...new Set(["scene", ...(spec.props ?? [])])] as (keyof (BuiltinProps & P) & string)[],
    events: [...new Set(["tap", "fps", "error", ...(spec.events ?? [])])] as (keyof (BuiltinEvents & E) & string)[],
  });

  return (props: GameViewProps<P, E>) => {
    let node: YNode | undefined;
    const user = props as GameViewProps<P, E> & Record<string, unknown>;
    const forward = (phase: TouchPhase, handler: string) => (e: GestureResponderEvent) => {
      for (const t of e.changedTouches) {
        node?.command("touch", { phase, id: t.identifier, x: t.locationX, y: t.locationY, t: t.timestamp });
      }
      (user[handler] as ((e: GestureResponderEvent) => void) | undefined)?.(e);
    };
    return (
      <Native
        {...(props as any)}
        ref={(n: YNode) => {
          node = n;
          (user.ref as ((n: YNode) => void) | undefined)?.(n);
        }}
        onTouchStart={forward("start", "onTouchStart")}
        onTouchMove={forward("move", "onTouchMove")}
        onTouchEnd={forward("end", "onTouchEnd")}
        onTouchCancel={forward("cancel", "onTouchCancel")}
        onError={(e: EngineError) => {
          const onError = user.onError as ((e: EngineError) => void) | undefined;
          onError ? onError(e) : console.error("nge 引擎错误", e.message);
        }}
      />
    );
  };
}
