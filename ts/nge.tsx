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
  /** 帧动画（只对图集有用）；有它时 `frame` 不起作用。节点别的字段变了不会从头播，`anim` 变了才从头播。 */
  anim?: AnimNode;
  /** 可点击：按下时发 tap 事件。 */
  tap?: boolean;
  /** 刚体。有了它位置归物理管：声明的 x / y / rotation 只在生成和改了的时候生效（同时重设初速度）。 */
  body?: BodyNode;
}

/** 精灵的刚体和碰撞体。尺寸是精灵本地的大小，会乘精灵的 scale。 */
export interface BodyNode {
  /** 默认 "dynamic"。kinematic 跟着声明的位置平滑移动，会推开别的刚体。 */
  kind?: "dynamic" | "fixed" | "kinematic";
  /** 默认 "box"。 */
  shape?: "box" | "circle";
  /** 碰撞体大小，不填取精灵的 w / h（圆取短边的一半）；都没有时只建刚体、不建碰撞体。 */
  w?: number;
  h?: number;
  r?: number;
  /** 弹性，默认 0。 */
  restitution?: number;
  /** 默认 0.5。 */
  friction?: number;
  /** 默认 1。 */
  density?: number;
  /** 只检测重叠，不碰撞。 */
  sensor?: boolean;
  /** 初速度（point / s）。 */
  velocity?: [number, number];
  /** 初角速度（度 / s）。 */
  angularVelocity?: number;
  /** 碰撞开始 / 结束时发 collide 事件。 */
  collide?: boolean;
  /** 不让碰撞把它转起来（平台游戏的角色）。 */
  fixedRotation?: boolean;
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

/**
 * 2D 相机。填了 `follow` 时相机跟着那个精灵，`x` / `y` 变成相对它的偏移；
 * `smooth` 是跟随的平滑时间（秒，0 每帧对准）；`bounds` 是相机能拍到的世界范围。
 */
export interface Camera2dNode {
  x?: number;
  y?: number;
  zoom?: number;
  follow?: string;
  smooth?: number;
  bounds?: { left: number; right: number; bottom: number; top: number };
}

/**
 * 瓦片地图。`data` 一行一行从上往下，每格是图集里的帧号（整数），负数是空格；`(x, y)` 是地图左上角（y 朝上，行往下排）。
 * `solid` 里的帧号是实心的，变成静态碰撞体；被撞时（地图或另一方开了 `collide`）发 `collide`，名字是地图的 id，
 * 同时碰到好几块只算一次接触。`cols` / `grid` / `solid` 也都是整数。
 * 任何字段变了整张地图重建，不要每帧改。
 */
export interface TilemapNode {
  id: string;
  /** 注册的图集名，或 asset:/ 路径配 `grid`。 */
  texture: string;
  grid?: [number, number];
  /** 每格的边长。 */
  tile: number;
  cols: number;
  data: number[];
  x?: number;
  y?: number;
  z?: number;
  solid?: number[];
  friction?: number;
  restitution?: number;
  collide?: boolean;
}

export interface Scene {
  /** 清屏色 "#rrggbb"。 */
  background?: string;
  camera?: Camera2dNode;
  sprites?: SpriteNode[];
  tilemaps?: TilemapNode[];
  texts?: TextNode[];
  emitters?: EmitterNode[];
  models?: ModelNode[];
  camera3d?: Camera3dNode;
  lights?: LightNode[];
  ambient?: { color?: string; brightness?: number };
  /** 重力（point / s²），默认 [0, -981]。 */
  physics?: { gravity?: [number, number] };
  audio?: AudioNode;
  /** 时间缩放，默认 1；0 暂停（物理、补间、动画、粒子、相机平滑都停），0.5 慢一半。声音不受影响。 */
  timeScale?: number;
}

/** 背景音乐和总音量。音量是线性的：0 静音，1 原音量。 */
export interface AudioNode {
  /** 循环播放的背景音乐（`asset:/` 路径）；换了就淡出旧的、淡入新的，去掉就淡出停止。 */
  music?: string;
  /** 默认 1。 */
  musicVolume?: number;
  /** 总音量，默认 1。 */
  volume?: number;
  muted?: boolean;
  /** 提前加载的声音，第一次播放时不用等。 */
  preload?: string[];
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

/** 两个精灵节点开始（started）或停止接触；其中一个的 body 开了 collide 才发。 */
export interface Collide {
  a: string;
  b: string;
  started: boolean;
}

/** 给精灵 `id` 的刚体加上 (x, y) 的速度（point / s，和质量无关）。 */
export function push(game: YNode | undefined, args: { id: string; x: number; y: number }) {
  game?.command("push", args);
}

export interface AnimNode {
  /** 依次播的图集格子序号。 */
  frames: number[];
  /** 默认 10。 */
  fps?: number;
  /** 默认 true；false 时停在最后一帧并发 animEnd。 */
  loop?: boolean;
}

export interface TweenProps {
  x?: number;
  y?: number;
  rotation?: number;
  scale?: number;
  alpha?: number;
}

export type Ease = "linear" | "easeIn" | "easeOut" | "easeInOut" | "backOut" | "bounceOut" | "elasticOut";

export interface Tween {
  /** 精灵节点的 id。 */
  id: string;
  /** 显式的起点；不填的属性从开始（delay 过后）时的当前值取。`to` 里没有的属性不起作用。 */
  from?: TweenProps;
  /** 变到这里；`rotation` 是度，`alpha` 替换颜色的透明度。写了别的字段整条命令无效。 */
  to: TweenProps;
  /** 秒，默认 0.3；不是正数时直接跳到终点。 */
  duration?: number;
  delay?: number;
  /** 默认 "easeOut"。 */
  ease?: Ease;
  /** 额外再播几次，-1 是一直播。 */
  repeat?: number;
  /** 配合 repeat 来回播：第二、四……次从 to 回到起点。 */
  yoyo?: boolean;
  /** 原样带回 tweenEnd 事件。 */
  tag?: string;
}

/**
 * 让精灵在一段时间里平滑地移动 / 旋转 / 缩放 / 渐隐，结束时发 tweenEnd。
 * 补间开始（delay 过后）时接管它写到的属性，属性全被接管的旧补间发 `cancelled: true` 的 tweenEnd；dynamic 刚体只补间 scale / alpha。
 */
export function tween(game: YNode | undefined, args: Tween) {
  game?.command("tween", args);
}

/** 震屏：幅度 `strength`（屏幕 point，默认 8）在 `duration` 秒（默认 0.3）里衰减到 0。 */
export interface Shake {
  strength?: number;
  duration?: number;
}

export function shake(game: YNode | undefined, args: Shake = {}) {
  game?.command("shake", args);
}

export interface AnimEnd {
  id: string;
}

export interface TweenEnd {
  id: string;
  tag: string | null;
  /** 被后开始的补间接管，没播完（一直播的补间被接管时也会发）。 */
  cancelled: boolean;
}

/** 播一次音效。`pitch` 是播放速度（2 = 高八度，限制在 0.1 到 8），`pan` 从 -1（左）到 1（右）。同时发声数有上限，超出的会被丢掉。 */
export function sound(game: YNode | undefined, args: { src: string; volume?: number; pitch?: number; pan?: number }) {
  game?.command("sound", args);
}

/** 飘一行字。`game` 是 `<GameView ref>` 拿到的节点。 */
export function floatText(game: YNode | undefined, args: FloatText) {
  game?.command("floatText", args);
}

/**
 * 一次按键（Mac）。`code` 是 Web `KeyboardEvent.code` 的键名（"KeyA"、"ArrowLeft"、"Space"），按键盘上的位置，和布局无关；手柄的按钮和摇杆方向是 "GamepadA"、"GamepadLeft"、"GamepadLeftStickRight" 这类（见 docs/gamepad.md）。
 * 键盘：输入框在打字、按着 ⌘ 时不发（手柄照样发）；切到别的窗口时按着的键（含手柄）都补发一次松开。
 */
export interface KeyInput {
  code: string;
  down: boolean;
  /** 按住不放时系统自动重复的按下。 */
  repeat: boolean;
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
  /**
   * Mac：游戏要吞掉的键（KeyInput.code），按下后不再交给 NSH。
   * 不填时是字母、数字、方向键、Space、Enter；用到别的键（如 Esc 暂停）要自己列上，不然 NSH 不处理时会「咚」一声。
   */
  captureKeys: string[];
}

interface BuiltinEvents {
  tap: Tap;
  collide: Collide;
  animEnd: AnimEnd;
  tweenEnd: TweenEnd;
  key: KeyInput;
  fps: FrameStats;
  error: EngineError;
}

type TouchPhase = "start" | "move" | "end" | "cancel";

export type GameViewProps<P, E> = NativeComponentProps<Partial<BuiltinProps> & P, BuiltinEvents & E>;

/**
 * 生成游戏视图组件。`props` / `events` 是游戏自己的通道名（Rust 侧 `add_prop` / `add_state` / `add_event`），
 * 类型由泛型给出；内置 prop `scene`、事件 `tap` / `collide` / `animEnd` / `tweenEnd` / `key` / `fps` / `error`，触摸自动转成引擎的 `touch` command。
 */
export function gameView<P extends object = {}, E extends object = {}>(
  spec: { props?: readonly (keyof P & string)[]; events?: readonly (keyof E & string)[] } = {},
) {
  const Native = nativeComponent<BuiltinProps & P, BuiltinEvents & E>("nge.game", {
    props: [...new Set(["scene", "captureKeys", ...(spec.props ?? [])])] as (keyof (BuiltinProps & P) & string)[],
    events: [...new Set(["tap", "collide", "animEnd", "tweenEnd", "key", "fps", "error", ...(spec.events ?? [])])] as (keyof (BuiltinEvents & E) & string)[],
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
