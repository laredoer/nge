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
  /** Rust 侧 `ScenePlugin` 注册的纹理名；不填是纯色矩形。 */
  texture?: string;
  /** 图集格子序号。 */
  frame?: number;
  /** 每秒转多少度。 */
  spin?: number;
  /** 可点击：按下时发 tap 事件。 */
  tap?: boolean;
}

export interface Scene {
  /** 清屏色 "#rrggbb"。 */
  background?: string;
  camera?: { x?: number; y?: number; zoom?: number };
  sprites: SpriteNode[];
}

/** 点中了可点击的精灵，x / y 是世界坐标。 */
export interface Tap {
  id: string;
  x: number;
  y: number;
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
