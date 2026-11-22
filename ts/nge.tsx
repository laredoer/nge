// nge 的 TSX 绑定：<GameView> 组件和场景类型（和 crates/nge-scene 的协议一一对应）。
import { nativeComponent, signal, Storage, Text, untracked, View, type GestureResponderEvent, type NativeComponentProps, type NativeComponentRef, type Signal } from "nsh";
import type { HttpReply } from "./http";
import type { Query3dReply } from "./physics3d";
import type { ActionEvent, ActionMap, PadEvent } from "./input";
import type { NetEvent, NetNode } from "./net";
import type { Prefab, PrefabInstance, PreloadEvent, TextureOptions, TransitionEvent, TransitionNode } from "./flow";
import { heldKey } from "./touch";

/** 一个精灵。坐标是世界坐标（point，y 朝上，原点在视口中心），角度用度（逆时针）。 */
export interface SpriteNode {
  /** 按它做增量同步；同一个场景里不能重复。 */
  id: string;
  x?: number;
  y?: number;
  /** 绘制顺序，大的在上面；小于 0 的是背景，画在 3D 模型后面（点击也是先模型后背景）。 */
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
  /**
   * 动画状态机（见 docs/anim-graph.md）：按它选图集的帧，有它时 `anim`、`frame` 不起作用。
   * 按值比较：每次算场景时新建一个值一样的对象不会重建；定义放在模块顶层的常量里，或者 `import` 一个 JSON 文件。
   */
  animGraph?: AnimGraphDef;
  /** 状态机的参数，和上一次声明不一样时才写进去（Rust 在中间设的不会被盖回去）。 */
  animParams?: AnimParams;
  /** 可点击：按下时发 tap 事件。 */
  tap?: boolean;
  /** 刚体。有了它位置归物理管：声明的 x / y / rotation 只在生成和改了的时候生效（同时重设初速度）。 */
  body?: BodyNode;
  /** 视差：相机移动时跟着挪的比例，默认 1。0.5 像远处的东西，0 钉在屏幕上，大于 1 像前景。`[x, y]` 两个方向分开，背景常用 `[0.3, 1]`（竖直方向跟着关卡）。按相机在原点时摆。 */
  parallax?: number | [number, number];
  /**
   * 沿世界的哪个轴一个挨一个重复铺满视野，步长是画出来的宽、高（含 scale，不管 rotation）。远山、云、无限长的地面用它，一个节点就够。
   * 只有原来那一份能点（`tap`）、有碰撞体（`body`）。见 docs/repeat.md。
   */
  repeat?: "x" | "y" | "xy";
  /** 用 `scene.shaders` 或内置的哪个精灵着色器画（flash、outline、wave、circle），见 docs/effects2d.md。不认识、编不过的按原样画。 */
  shader?: string;
  /** 交给着色器的 4 个数（`e.params`），不写用着色器的默认值。 */
  params?: [number, number, number, number];
  /** 当遮罩：不画出来，只记下不透明（alpha ≥ 0.5）的地方。z < 0 和 z ≥ 0 的精灵各用各的遮罩。 */
  mask?: boolean;
  /** 只画在遮罩里（"inside"）或遮罩外（"outside"）。 */
  masked?: "inside" | "outside";
  /** 开着 2D 光照时不受光（自己发光的东西）。 */
  unlit?: boolean;
  /**
   * 法线图（`asset:/` 路径或注册过的纹理名，不能是图集），和 `texture` 同样排布：RGB = 法线 × 0.5 + 0.5，
   * 绿色朝上、蓝色朝外，按原值读（不做 sRGB 转换）。受光时按表面朝向明暗。
   */
  normal?: string;
  /** 挡住开了 `shadows` 的灯：`true` 是精灵画出来的矩形，数组是本地坐标的多边形（至少 2 个点，首尾自动连上）。挡光的精灵自己照常亮。 */
  occluder?: boolean | [number, number][];
  /**
   * 追某个节点：看见（`sight` 以内、中间没有墙）就在瓦片地图里绕开墙追过去，目标跑出 `sight` 的 1.5 倍、
   * 自己离出发点超过 `leash`、找不到路就回出发点。直接挪位置，配 `kinematic` 刚体。见 docs/ai.md。
   */
  chase?: ChaseNode;
  /**
   * 行为树（见 docs/gameplay2d.md）：每帧跑一遍，叶子可以按地面寻路追目标（走、跳上平台、落下、从单向平台穿下去）。
   * 要配 `body: { kind: "character" }`。树变了才重建，每帧声明同一棵树不会重置。写错时只丢 `behavior`，精灵照样有。
   */
  behavior?: BehaviorNode;
}

/** `behavior`：树、黑板的初值、走多快（像素 / 秒，默认 120）、起跳速度（像素 / 秒，默认 0 不会跳）。 */
export interface BehaviorNode {
  tree: BtNode;
  blackboard?: Record<string, BlackboardValue>;
  speed?: number;
  jump?: number;
}

/** 黑板上的值：布尔、数、字符串、点 `[x, y]`。 */
export type BlackboardValue = boolean | number | string | [number, number];

/**
 * 行为树的节点。`sequence` 一个一个跑、有失败就失败；`selector` 一个一个试、有成就成；
 * `reactive` 的每帧从第一个重新查（条件变了能打断后面在跑的）。`parallel` 成了 `succeed` 个就成；
 * `repeat` 成了再跑（`times` 次，不写一直跑）；`cooldown` 孩子跑完后 `seconds` 秒内直接失败；`wait` 等几秒。
 * 叶子：`see` 在 `range` 以内且没墙挡着；`near` 在 `range` 以内；`check` 黑板上的值是真的；`set` 写黑板（`null` 删掉）；
 * `chase` / `goto` / `home` 沿地面走过去（到了成、走不到败、路上在跑）；`emit` 发 `behavior` 事件。
 */
export type BtNode =
  | { type: "sequence" | "selector"; children: BtNode[]; reactive?: boolean }
  | { type: "parallel"; succeed: number; children: BtNode[] }
  | { type: "invert"; child: BtNode }
  | { type: "repeat"; times?: number; child: BtNode }
  | { type: "cooldown"; seconds: number; child: BtNode }
  | { type: "wait"; seconds: number }
  | { type: "see" | "near"; target: string; range: number }
  | { type: "check"; key: string }
  | { type: "set"; key: string; value: BlackboardValue | null }
  | { type: "chase"; target: string }
  | { type: "goto"; x: number; y: number }
  | { type: "home" }
  | { type: "emit"; name: string };

/** 拼行为树的小函数：`bt.selector([bt.sequence([bt.see("hero", 200), bt.chase("hero")], true), bt.home()], true)`。 */
export const bt = {
  sequence: (children: BtNode[], reactive = false): BtNode => ({ type: "sequence", children, reactive }),
  selector: (children: BtNode[], reactive = false): BtNode => ({ type: "selector", children, reactive }),
  parallel: (succeed: number, children: BtNode[]): BtNode => ({ type: "parallel", succeed, children }),
  invert: (child: BtNode): BtNode => ({ type: "invert", child }),
  repeat: (child: BtNode, times?: number): BtNode => (times === undefined ? { type: "repeat", child } : { type: "repeat", times, child }),
  cooldown: (seconds: number, child: BtNode): BtNode => ({ type: "cooldown", seconds, child }),
  wait: (seconds: number): BtNode => ({ type: "wait", seconds }),
  see: (target: string, range: number): BtNode => ({ type: "see", target, range }),
  near: (target: string, range: number): BtNode => ({ type: "near", target, range }),
  check: (key: string): BtNode => ({ type: "check", key }),
  set: (key: string, value: BlackboardValue | null): BtNode => ({ type: "set", key, value }),
  chase: (target: string): BtNode => ({ type: "chase", target }),
  goto: (x: number, y: number): BtNode => ({ type: "goto", x, y }),
  home: (): BtNode => ({ type: "home" }),
  emit: (name: string): BtNode => ({ type: "emit", name }),
};

/** 行为树里的 `emit`：哪个节点、什么名字、当时在哪。 */
export interface BehaviorEvent {
  id: string;
  name: string;
  x: number;
  y: number;
}

export interface ChaseNode {
  /** 追哪个节点（id）。 */
  target: string;
  /** 每秒多少像素，默认 120。 */
  speed?: number;
  /** 多远能看见，默认 240。 */
  sight?: number;
  /** 离出发点多远就不追了，默认 480。 */
  leash?: number;
}

/** `chase` 的状态变了：`idle` 在出发点等着，`chase` 在追，`return` 在回出发点。 */
export interface ChaseEvent {
  id: string;
  state: "idle" | "chase" | "return";
  x: number;
  y: number;
}

/** 2D 光照：写了就开，受光的精灵颜色乘上环境光加上照到它的灯。粒子、文字不受光。 */
export interface LightingNode {
  /** 环境光，默认 "#ffffff"（不变暗）。 */
  ambient?: string;
  /** 最多 32 盏。 */
  lights?: Light2dNode[];
}

/** 2D 点光源：中心最亮，到 `radius` 减到 0。 */
export interface Light2dNode {
  /** `follow` 时是相对那个精灵的偏移。 */
  x?: number;
  y?: number;
  radius: number;
  /** 默认 "#ffffff"。 */
  color?: string;
  /** 默认 1。 */
  intensity?: number;
  /** 跟着这个精灵（有刚体时是插值过的位置）。 */
  follow?: string;
  /** 离画面多高，只影响带 `normal` 的精灵：越低越像从侧面打过来。默认半径的 0.25 倍。 */
  height?: number;
  /**
   * 被 `occluder` 的精灵挡住，后面落在阴影里（硬阴影）。默认 false。
   * 开销是受光的像素 × 开了 shadows 的灯 × 挡光的边数，灯和挡光物都少放几个。
   */
  shadows?: boolean;
}

/**
 * 后处理的一遍：`shader` 是 `scene.shaders` 或内置的后处理（grayscale、vignette、pixelate、tonemap、fxaa），
 * 或者内置的多遍效果 `bloom`（亮的地方往外晕开，params 是 [阈值 0.8, 强度 1, 级数 5, 0]）。
 */
export interface PostNode {
  shader: string;
  /** 交给着色器的 4 个数（`p.params`），不写用着色器的默认值。 */
  params?: [number, number, number, number];
  /**
   * "back" 只给 z < 0 的背景层（要背景层第一个画）。和 3D 混排时也照样分出这一层，在 3D 之前画，所以这些精灵总在 3D 后面。
   * 默认 "screen"（整屏）。
   */
  layer?: "screen" | "back";
}

/** 精灵的刚体和碰撞体。尺寸是精灵本地的大小，会乘精灵的 scale。 */
export interface BodyNode {
  /**
   * 默认 "dynamic"。kinematic 跟着声明的位置平滑移动，会推开别的刚体。
   * character 是角色控制器：Rust 改速度，引擎负责重力、贴地、沿墙滑、爬坡、上台阶、被脚下的平台带着走（见 docs/character.md）。
   */
  kind?: "dynamic" | "fixed" | "kinematic" | "character";
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
  /** 单向平台：只挡从上面来的，从下面、侧面穿过去；角色按「下」加跳穿下去（见 docs/gameplay2d.md）。 */
  oneWay?: boolean;
  /** 初速度（point / s）。 */
  velocity?: [number, number];
  /** 初角速度（度 / s）。 */
  angularVelocity?: number;
  /** 碰撞开始 / 结束时发 collide 事件。 */
  collide?: boolean;
  /** 不让碰撞把它转起来（平台游戏的角色）。 */
  fixedRotation?: boolean;
  /** 碰撞分组（1 … 31）：同一组的互相不碰撞（两个玩家的角色互相穿过），和别的照常碰。不填不分组。 */
  group?: number;
  /** character：能爬的最陡的坡（度），默认 50。 */
  slope?: number;
  /** character：自动迈上去的台阶最高多高，默认 0（不自动上）。 */
  step?: number;
  /** character：离地多近就往下贴，默认 4（0 不贴）。 */
  snap?: number;
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

/** 进度条（血条），见 docs/world-ui.md。 */
export interface BarNode {
  id: string;
  /** 0 到 1，超出截断；不写或 NaN 当 0。 */
  value?: number;
  /** 跟着这个精灵或 3D 模型（id）画的位置走，x / y 变成相对它的偏移；找不到或在 3D 相机背后就藏起来。 */
  follow?: string;
  /** 跟随 3D 模型时在 3D 世界里往上抬的高度。 */
  lift?: number;
  x?: number;
  y?: number;
  /** 默认 32 × 4。 */
  w?: number;
  h?: number;
  /** 填充色，默认 "#4ade80"。 */
  color?: string;
  /** 底色，默认 "#000000a0"。 */
  back?: string;
  /** 默认 50。 */
  z?: number;
  /** 数值变化的过渡时间（秒），默认 0.15，0 是立刻。 */
  smooth?: number;
}

/** 带背景框的一段文字：背景框按文字排版后的大小撑开。 */
export interface BubbleNode {
  id: string;
  text: string;
  /** 跟着这个精灵或 3D 模型走，x / y 是相对它的偏移；位置是气泡的底边中点。 */
  follow?: string;
  /** 跟随 3D 模型时在 3D 世界里往上抬的高度。 */
  lift?: number;
  x?: number;
  y?: number;
  /** 固定在屏幕上（对话框）：x / y 是屏幕坐标（左上角原点，y 朝下），位置是气泡的顶边中点。 */
  screen?: boolean;
  /** 字号，默认 14。 */
  size?: number;
  /** 字的颜色，默认白色。 */
  color?: string;
  /** 背景框颜色，默认 "#000000b0"。 */
  back?: string;
  /** 背景框比文字大出来的边，默认 6。 */
  padding?: number;
  /** 超过就换行，默认 160。 */
  maxWidth?: number;
  /** 每秒出几个字，默认 0（一下全出来）。改了文字从头打，打完发 `typed`；改成 0 一下全出来。 */
  typing?: number;
  font?: string;
  /** 背景框的 z，默认 60；字总在所有精灵上面。 */
  z?: number;
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
  /**
   * 跟着这个精灵（通常是 2D 刚体）走和转，见 docs/joints.md。这时 x、y、z 是精灵坐标里的偏移（x、y 跟着精灵转），
   * `rotation` 在精灵的角度上再转，`spin` 不起作用。精灵不存在时模型留在原地。
   */
  attach?: string;
  /** 挂到另一个 3D 模型的指定骨骼/节点上：x/y/z、rotation、scale 变为相对骨骼局部坐标系的偏移，随目标骨骼动画实时运动。 */
  socket?: SocketNode;
  /** 播模型里的一段骨骼 / 节点动画（glTF 的 animations），见 docs/3d-more.md。只改 speed、loop 时不从头播。 */
  animation?: ModelAnimation;
  /** 动画状态机（见 docs/anim-graph.md）：片段是模型里的动画名，有它时 `animation` 不起作用。按值比较，定义变了才重建。 */
  animGraph?: AnimGraphDef;
  /** 状态机的参数，和上一次声明不一样时才写进去。 */
  animParams?: AnimParams;
  /** 3D 刚体（见 docs/physics3d.md）。有了它位置和旋转归物理管：声明的位置、角度、缩放没变时不拉回去；`attach`、`socket`、`spin` 不生效。 */
  body?: ModelBody;
  /** 拖尾（见 docs/particles3d.md）：模型走过的地方留一条朝着相机的带子。去掉时马上消失；只改参数时保留已经记下的点。 */
  trail?: TrailNode;
  /** 材质着色风格覆盖："pbr"（默认标准 PBR）| "toon"（卡通分阶 + 边缘光）| "water"（动态水波 + 菲涅尔）| "foliage"（风吹摆动 + 逆光透射）。 */
  style?: "pbr" | "toon" | "water" | "foliage";
  /** 使用 `scene.shaders3d` 里注册的自定义 3D 着色器（WGSL 片段定义 `fn shade3d(s: Surface) -> vec4f`）。 */
  shader?: string;
  /** 传给自定义 3D 着色器的 4 个浮点参数（`s.params`）。 */
  params?: [number, number, number, number];
  /** 3D 导航网格寻路代理：跟随目标 3D 模型、沿路点循环巡逻或前往指定 3D 坐标。 */
  nav?: NavAgent3dNode;
  /** 程序化骨骼注视：让指定骨骼在动画基础上平滑转向目标 3D 模型或世界坐标。 */
  lookAt?: LookAtNode;
}

/** `ModelNode.lookAt`：3D 程序化骨骼注视配置。 */
export interface LookAtNode {
  /** glTF 骨骼/节点名（如 "head"、"body"、"armR"）。 */
  bone: string;
  /** 注视的目标 3D 模型 id（与 point 二选一）。 */
  target?: string;
  /** 注视的固定 3D 世界坐标 `[x, y, z]`。 */
  point?: [number, number, number];
  /** 混合权重 0–1，默认 1。 */
  weight?: number;
  /** 最大偏转角（度），默认 60。 */
  maxAngle?: number;
  /** 转向平滑速度（弧度 / 秒），默认 8。 */
  speed?: number;
}

/** `scene.navmesh3d`：从指定 3D 地面模型与静态碰撞体自动构建 3D 导航网格。 */
export interface NavMesh3dNode {
  /** 用于提取可行走坡面的 3D 模型 id（如 "terrain"）。 */
  surface: string;
  /** 最大可行走坡度（度），默认 45。 */
  maxSlope?: number;
  /** 最低可行走海拔（例如过滤海平面以下），默认不限制。 */
  minY?: number;
  /** 角色避障膨胀半径，默认 12。 */
  agentRadius?: number;
  /** 是否用 3D 调试线实时绘制导航路径，默认 false。 */
  debug?: boolean;
}

/** `ModelNode.nav`：3D 导航寻路代理配置。 */
export interface NavAgent3dNode {
  /** 跟随目标 3D 模型的 id。 */
  follow?: string;
  /** 循环巡逻的 3D 路点列表。 */
  patrol?: [number, number, number][];
  /** 前往的单个 3D 目标坐标。 */
  target?: [number, number, number];
  /** 移动速度（世界单位 / 秒），默认 95。 */
  speed?: number;
  /** 跟随模式下与目标保持的最小距离，默认 48。 */
  minDistance?: number;
  /** 到达路点的判定距离，默认 16。 */
  arrive?: number;
  /** 离导航地面的垂直高度偏移，默认 0。 */
  hover?: number;
  /** 转向平滑速度（弧度 / 秒），默认 7。 */
  turnSpeed?: number;
  /** 巡逻路点停留秒数，默认 1.2。 */
  wait?: number;
}

/** 3D 骨骼挂点：把当前模型挂到目标 3D 模型的指定骨骼/节点上。 */
export interface SocketNode {
  /** 目标 3D 模型的 id。 */
  target: string;
  /** glTF 节点名（如 "armR"、"hand_r"）；留空时跟在目标模型根节点上。 */
  bone?: string;
}

/** 混合方式：alpha 普通透明（从远往近画），add 叠加（火、光点）。 */
export type Blend = "alpha" | "add";

/** 模型的拖尾。 */
export interface TrailNode {
  /** 一个点留多久（秒），默认 0.5。 */
  lifetime?: number;
  /** [新, 旧] 两头的宽，默认 [0.2, 0]。 */
  width?: [number, number];
  /** [新, 旧]；不填是白色淡出。 */
  color?: [string, string];
  /** 相邻两个点至少隔多远，默认 0。静止时尾巴缩回去。 */
  minDistance?: number;
  /** 最多记多少个点，默认 64。 */
  maxPoints?: number;
  /** `asset:/` 路径或注册过的纹理名（不支持图集）。 */
  texture?: string;
  blend?: Blend;
}

/** 3D 贴地光环与贴花（`scene.decals3d`）：内置 `ring` / `rune` / `pulse` / `shadow` 程序化纹理，自动贴合地形起伏。 */
export interface Decal3dNode {
  id: string;
  /** 内置图案："ring"（光环）| "rune"（古代法阵）| "pulse"（扩散波纹）| "shadow"（柔和脚底影）。默认 "ring"。 */
  kind?: "ring" | "rune" | "pulse" | "shadow" | (string & {});
  /** 跟随目标 3D 模型的 id；不写时使用 (x, y, z) 固定世界坐标。 */
  follow?: string;
  x?: number;
  y?: number;
  z?: number;
  /** 贴花直径（世界单位），默认 40。 */
  size?: number;
  /** 颜色（"#rrggbb" / "#rrggbbaa"）。 */
  color?: string;
  /** 自转速度（度 / 秒），默认 0。 */
  spin?: number;
  /** 呼吸脉冲缩放幅度（0–1），默认 0。 */
  pulse?: number;
  /** 混合方式："add"（发光叠加）或 "alpha"（普通透明）。 */
  blend?: Blend;
  /** 是否按 `NavMesh3d` 自动采样地面高度贴地，默认 true。 */
  ground?: boolean;
  /** 离地微抬高度（默认 2.5，防止与地形 Z-fighting）。 */
  lift?: number;
}

/** 3D 粒子发射器（见 docs/particles3d.md）：粒子是朝着相机的方片，被模型挡住，自己不挡别的。 */
export interface Emitter3dNode {
  id: string;
  x?: number;
  y?: number;
  z?: number;
  /** 每秒发射多少个，默认 0。 */
  rate?: number;
  /** 寿命（秒），默认 [1, 1]。 */
  lifetime?: [number, number];
  /** 初速度（世界单位 / 秒），默认 [1, 2]。 */
  speed?: [number, number];
  /** 发射方向，默认 [0, 1, 0]（朝上）。 */
  direction?: [number, number, number];
  /** 方向锥的半角（度），默认 180（全方向）。 */
  spread?: number;
  /** 加速度 [x, y, z]。 */
  gravity?: [number, number, number];
  /** [出生, 死亡] 时的边长，默认 [0.2, 0]。 */
  size?: [number, number];
  /** 方片转动的速度（度 / 秒），出生时在范围里取。 */
  spin?: [number, number];
  /** [出生, 死亡]；不填是白色淡出。 */
  color?: [string, string];
  /** `asset:/` 路径或注册过的纹理名（不支持图集）；不填是方片。 */
  texture?: string;
  blend?: Blend;
  /** 同时存活的上限，默认 10000。 */
  max?: number;
  // 下面是 3D 粒子补齐（见 docs/particles3d-more.md）：写错的字段 warn 一次后当没写，别的照用；多余的键也 warn。
  /** 出生时的转角范围（度）；不写时一整圈随机（序列帧的火苗要正着就写 [0, 0]）。 */
  angle?: [number, number];
  /** 在哪出生（本地坐标），默认一个点。 */
  shape?: EmitShapeNode;
  /** 发射方向：`cone` 用 direction + spread（默认），`normal` 沿出生处的法线（再按 spread 散开），`random` 全方向均匀。 */
  emitDirection?: "cone" | "normal" | "random";
  /** 大小随寿命 `[t, 边长][]`，t 是年龄 / 寿命；设了就不用 size。 */
  sizeCurve?: [number, number][];
  /** 透明度随寿命，乘到颜色的透明度上。 */
  alphaCurve?: [number, number][];
  /** 速度随寿命，乘到每帧的位移上。 */
  speedCurve?: [number, number][];
  /** 颜色随寿命 `[t, "#rrggbbaa"][]`；设了就不用 color。 */
  colorGradient?: [number, string][];
  /** 速度阻尼：每秒乘 e^-drag。 */
  drag?: number;
  /** 湍流（curl noise）：strength 是加速度（默认 1），frequency 每单位几格（默认 1），scroll 每秒沿 y 移几格（默认 0）。 */
  noise?: { strength?: number; frequency?: number; scroll?: number };
  /** 朝向：朝相机（默认）、沿速度拉长、水平躺着，或固定朝向的欧拉角 [x, y, z]（度，和模型的 rotation 一样先绕 x、再绕 y、最后绕 z）。 */
  facing?: "camera" | "velocity" | "horizontal" | [number, number, number];
  /** `facing: "velocity"` 时的拉长系数：长 = 边长 + 速度 × stretch，默认 0.1。 */
  stretch?: number;
  /** 序列帧：grid 是 [列, 行]，从左上角按行数；不写 fps 时按寿命播完一遍。 */
  sheet?: { grid: [number, number]; frames?: number; fps?: number; randomStart?: boolean; blend?: boolean };
  /** 软粒子：离后面的不透明表面不到这么远时淡出（GL 后端上没有，退成硬边）。 */
  soft?: number;
  /** 受环境光和方向光影响。 */
  lit?: boolean;
  /**
   * 碰撞：地面高度 ground、场景碰撞体 scene；bounce 反弹（默认 0.5）、friction 摩擦、kill 碰到就死、radius 粒子半径。
   * group 跳过这个碰撞分组的碰撞体（同 ModelBody.group），only 只碰这些分组（0 是不分组的）；出生在碰撞体里的粒子不被它挡。
   * 贴着表面（停着、滑着）时只在刚碰上那一帧算一次碰撞。
   */
  collision?: { ground?: number; scene?: boolean; bounce?: number; friction?: number; kill?: boolean; radius?: number; group?: number; only?: number[] };
  /** 子发射器：父粒子出生、死亡、碰撞时在它的位置 burst count 个（最多 10000）；最多套 3 层。按下标对应，调换顺序会换掉已经发出去的子粒子。 */
  subEmitters?: SubEmitterNode[];
}

/** 发射形状；角度用度。 */
export type EmitShapeNode =
  | { kind: "point" }
  | { kind: "sphere" | "hemisphere"; radius: number; surface?: boolean }
  | { kind: "box"; size: [number, number, number]; surface?: boolean }
  /** 本地 xz 平面上的圆环，inner = 0 是圆盘，inner = radius 是一圈线。 */
  | { kind: "circle"; radius: number; inner?: number }
  /** 底面半径 radius 的圆盘，越靠边越往外偏，最外圈偏 angle。 */
  | { kind: "cone"; radius: number; angle: number }
  /** glTF 模型的表面，按三角形面积取点。 */
  | { kind: "mesh"; model: string; scale?: number | [number, number, number] };

/** 子发射器。`emitter` 的 rate、id、位置不起作用；碰撞时本地 +Y 对着碰到的面的法线。 */
export interface SubEmitterNode {
  on: "birth" | "death" | "collision";
  count: number;
  /** 继承父粒子速度的比例，默认 0。 */
  inherit?: number;
  emitter: Omit<Emitter3dNode, "id" | "x" | "y" | "z">;
}

/** 模型的 3D 刚体和碰撞体。和 2D 刚体在两个世界里，互不碰撞。 */
export interface ModelBody {
  /**
   * 默认 "dynamic"。kinematic 跟着声明的位置平滑移动，会推开别的刚体。
   * character 是角色控制器：游戏在 Rust 里改速度，引擎负责重力、贴地、爬坡、上台阶、被平台带着走（见 docs/gameplay3d.md）。
   */
  kind?: "dynamic" | "fixed" | "kinematic" | "character";
  /**
   * 默认 "box"。capsule 是竖着的胶囊。
   * convex / hulls / trimesh 从模型网格生成（按 scale 缩放，模型加载完才建刚体）：整个模型一个凸包、每个图元一个凸包、三角网格（只能配 kind: "fixed"）。
   */
  shape?: "box" | "ball" | "capsule" | "convex" | "hulls" | "trimesh";
  /** 盒子的长宽高（世界单位，不乘 scale），默认边长 = 模型的 scale。 */
  size?: [number, number, number];
  /** 球、胶囊的半径，默认 scale / 2（球）、scale / 4（胶囊）。 */
  r?: number;
  /** 胶囊中间圆柱的高，默认 scale / 2。 */
  h?: number;
  /** 弹性，默认 0。 */
  restitution?: number;
  /** 默认 0.5。 */
  friction?: number;
  /** 默认 1。 */
  density?: number;
  /** 只检测重叠，不碰撞。 */
  sensor?: boolean;
  /** 初速度（point / s）。 */
  velocity?: [number, number, number];
  /** 碰撞开始 / 结束时发 collide 事件（两边都是模型时）。 */
  collide?: boolean;
  /** 碰撞分组（1 … 31）：同一组的互相不碰撞；0 不分组。 */
  group?: number;
  /** character：能爬的最陡的坡（度），默认 50。 */
  slope?: number;
  /** character：自动迈上去的台阶最高多高，默认 0 不自动上。 */
  step?: number;
  /** character：离地多近就往下贴，默认 4，0 不贴。 */
  snap?: number;
}

export interface ModelAnimation {
  /** 动画名；glTF 里没名字的按编号叫（"0"、"1"…）。找不到时停在不播动画的姿势。 */
  clip: string;
  /** 播放速度，默认 1；负数倒着播。 */
  speed?: number;
  /** 循环，默认 true；false 时停在最后一帧。 */
  loop?: boolean;
  /** 换 `clip` 时从旧动画淡过去的秒数（骨骼和变形权重一起混），默认 0 硬切；见 docs/render3d-more.md。 */
  fade?: number;
}

/**
 * 关节：把两个带 body 的精灵（或一个精灵和世界）连起来，见 docs/joints.md。
 * 锚点、轴都是世界坐标，按两头精灵**声明的**位置和角度换算；精灵后来才有、删掉了，关节跟着连上、拆掉。
 */
export interface JointNode {
  id: string;
  /** 精灵 id；不写时连到世界上。 */
  a?: string;
  b: string;
  /** fixed 焊死；revolute 绕支点转；prismatic 沿 axis 滑、不转；rope 最远 length；spring 拉向 length。 */
  kind: "fixed" | "revolute" | "prismatic" | "rope" | "spring";
  /**
   * fixed / revolute / prismatic 只有一个支点：只写 anchorA 或 anchorB 时两头都用它，都不写是 b 的位置。
   * rope / spring 两头各一个：anchorA 不写是 a 的位置（连世界时必须写），anchorB 不写是 b 的位置。
   */
  anchorA?: [number, number];
  anchorB?: [number, number];
  /** prismatic 滑的方向，默认 [1, 0]。 */
  axis?: [number, number];
  /** revolute：b 相对 a 能转的角度范围（度，从声明时算起）；prismatic：沿轴的范围（point，从支点算起）。 */
  limits?: [number, number];
  /** 马达：revolute 的 speed 是度 / 秒，prismatic 是 point / s；maxForce 不写时不限。 */
  motor?: { speed: number; maxForce?: number };
  /** rope 的最大长度、spring 的静止长度；不写时是两个锚点声明时的距离。 */
  length?: number;
  /** spring 的劲度，按加速度算、和质量无关，默认 100。 */
  stiffness?: number;
  /** spring 的阻尼，默认 10。 */
  damping?: number;
  /** 连着的两个精灵之间还碰撞，默认不碰。 */
  collide?: boolean;
}

/**
 * 3D 关节：把两个带 body 的模型（或一个模型和世界）连起来，见 docs/gameplay3d.md。字段和 `JointNode` 一样，坐标是三维的。
 */
export interface Joint3dNode {
  id: string;
  /** 模型 id；不写时连到世界上。 */
  a?: string;
  b: string;
  /** fixed 焊死；ball 球窝，绕支点随便转；revolute 铰链，绕 axis 转；prismatic 沿 axis 滑、不转；rope 最远 length；spring 拉向 length。 */
  kind: "fixed" | "ball" | "revolute" | "prismatic" | "rope" | "spring";
  /**
   * fixed / ball / revolute / prismatic 只有一个支点：只写 anchorA 或 anchorB 时两头都用它，都不写是 b 的位置。
   * rope / spring 两头各一个：anchorA 不写是 a 的位置（连世界时必须写），anchorB 不写是 b 的位置。
   */
  anchorA?: [number, number, number];
  anchorB?: [number, number, number];
  /** revolute 绕的轴、prismatic 滑的方向（世界坐标），默认 [1, 0, 0]。 */
  axis?: [number, number, number];
  /** revolute：b 相对 a 能转的角度范围（度，从声明时算起）；prismatic：沿轴的范围（point，从支点算起）。 */
  limits?: [number, number];
  /** 马达：revolute 的 speed 是度 / 秒，prismatic 是 point / s；maxForce 不写时不限。 */
  motor?: { speed: number; maxForce?: number };
  /** rope 的最大长度、spring 的静止长度；不写时是两个锚点声明时的距离。 */
  length?: number;
  /** spring 的劲度，按加速度算、和质量无关，默认 100。 */
  stiffness?: number;
  /** spring 的阻尼，默认 10。 */
  damping?: number;
  /** 连着的两个模型之间还碰撞，默认不碰。 */
  collide?: boolean;
}

/** 3D 相机：在 (x, y, z) 看向 target。不填时在 (0, 2, 6) 看向原点。 */
export interface Camera3dNode {
  x?: number;
  y?: number;
  /** 默认 6；`match2d` 时是相机离 z = 0 平面的距离，默认 1000，模型的 z 在 [-z, z] 之间都看得到。 */
  z?: number;
  target?: [number, number, number];
  /** 垂直视角（度），默认 60。 */
  fov?: number;
  /** 正交投影：竖直方向看到这么多个世界单位，远近一样大。 */
  ortho?: number;
  /** 每帧跟着 2D 相机走（跟随、震屏都跟上）：正交，3D 的一个单位是 2D 的一个 point，模型放在 (x, y, 0) 就在 2D 世界的 (x, y)。开着时 `x`、`y`、`target`、`fov`、`ortho` 都不看。 */
  match2d?: boolean;
  /** 模型和精灵按深度混排：模型后面的精灵被挡住、前面的露出来（精灵的 z 当作 3D 的 z），见 docs/3d-more.md。默认关：z < 0 的精灵画在模型下面，其余画在上面。 */
  interleave?: boolean;
  /** 方向光的影子分几段（1–4，近处的段更清楚），默认 1；见 docs/render3d-more.md。 */
  shadowCascades?: number;
  /** 分段的影子覆盖到相机前方多远（3D 单位，不超过相机的远平面），默认 50。 */
  shadowDistance?: number;
  /** 弹簧臂跟随目标 3D 模型的 id；写了以后自动平滑跟随目标并可防穿墙。 */
  follow?: string;
  /** 弹簧臂距离（默认 180）。 */
  distance?: number;
  /** 弹簧臂俯仰角（度，正值俯视，默认 22）。 */
  pitch?: number;
  /** 弹簧臂水平偏航角（度）；不填时自动跟随目标模型朝向。 */
  yaw?: number;
  /** 注视点在目标上方的抬高高度（默认 24）。 */
  lift?: number;
  /** 跟随平滑时间（秒，默认 0.12，0 为瞬间对齐）。 */
  smooth?: number;
  /** 弹簧臂防穿墙检测（默认 true，撞到地形/墙体时自动拉近相机）。 */
  collision?: boolean;
}

export interface LightNode {
  id: string;
  kind: "directional" | "point" | "spot";
  /** 方向光、聚光灯照射的方向，默认朝下。 */
  direction?: [number, number, number];
  /** 点光源、聚光灯的位置。 */
  x?: number;
  y?: number;
  z?: number;
  color?: string;
  /** 方向光的照度 / 点光源、聚光灯的强度。 */
  intensity?: number;
  /** 点光源、聚光灯的照射范围。 */
  range?: number;
  /** 聚光灯外锥的半角（度），默认 30；外面是黑的。 */
  angle?: number;
  /** 聚光灯内锥的半角（度），默认外锥的 80%；里面全亮，和外锥之间平滑过渡。 */
  innerAngle?: number;
  /** 投影子，见 docs/3d-more.md、docs/render3d-more.md；聚光灯、点光源最多 `shadows.maxLights` 盏投（见 docs/render3d-quality.md）。 */
  shadows?: boolean;
  /** 投影子的聚光灯、点光源超过上限时先挑大的，一样大挑离相机近的；没挑上的照常照明。要是整数，写小数 warn 后当没写。 */
  shadowPriority?: number;
}

/** 天空和环境光照（见 docs/render3d-quality.md）。写了 `cube` 用立方体贴图，否则是程序天空。 */
export interface SkyNode {
  /** 一张 KTX2 立方体贴图，或者 +X、-X、+Y、-Y、+Z、-Z 六张图（从里面朝那个方向看到的画面）。 */
  cube?: string | [string, string, string, string, string, string];
  top?: string;
  horizon?: string;
  ground?: string;
  /** 阳光照射的方向（和方向光一样）；不写跟第一盏方向光，[0, 0, 0] 是没有太阳。 */
  sun?: [number, number, number];
  sunColor?: string;
  /** 太阳盘的半角（度），默认 1.5。 */
  sunSize?: number;
  /** 环境漫反射、镜面反射强度，默认 1。 */
  diffuse?: number;
  specular?: number;
  /** 画天空，默认 true；false 时只拿来照明。 */
  background?: boolean;
}

/** 雾。不写 `mode` 时，写了 `start` / `end` 是线性、写了 `density` 是指数。 */
export interface FogNode {
  mode?: "none" | "linear" | "exp" | "exp2";
  /** 线性雾：`start` 以内没有雾，到 `end` 最浓；默认 0、100。 */
  start?: number;
  end?: number;
  /** 指数雾的密度，默认 0.02。 */
  density?: number;
  color?: string;
  /** 0–1：雾色往视线方向的天空色混多少。 */
  sky?: number;
  /** 高度雾：越低越浓，`density × e^(-falloff × (高度 - base))`。 */
  height?: { base?: number; falloff?: number; density?: number };
  /** 最浓多少（0–1），默认 1。 */
  max?: number;
}

/** 阴影的过滤和投影灯数；分几段、覆盖多远在 `camera3d` 里写。 */
export interface ShadowsNode {
  /** 默认 "pcf"（3×3）；"poisson" 泊松盘；"pcss" 方向光按挡板远近变虚（聚光灯、点光源按泊松盘）。 */
  filter?: "pcf" | "poisson" | "pcss";
  /** 泊松盘半径（阴影图像素），默认 1.5。 */
  softness?: number;
  /** PCSS 的太阳角直径（度），默认 1，最大 10；越大半影越宽。 */
  sunSize?: number;
  /** 深度偏移（光空间深度），默认 0。 */
  bias?: number;
  /** 法线偏移（阴影图像素），默认 1.5。 */
  normalBias?: number;
  /** 聚光灯、点光源最多几盏投影子，默认 2，最多 4。要是整数，写小数 warn 后当没写。 */
  maxLights?: number;
  /** 级联交界的过渡带占一段的多少（0–0.5），默认 0.1。 */
  blend?: number;
  /** 按级联染色：红、绿、蓝、黄。 */
  debugCascades?: boolean;
  /** 方向光阴影图的边长，默认 2048。要是整数，写小数 warn 后当没写。 */
  size?: number;
  /** 聚光灯、点光源阴影图的边长，默认 512；显存超过 64 MB 时自动减半。要是整数，写小数 warn 后当没写。 */
  localSize?: number;
}

/** 3D 真实线性 HDR 色调映射（在写入 8-bit 帧缓冲前压缩高光，避免过曝发白）。 */
export interface ToneMapNode {
  /** 曲线模式，写 `tonemap` 时默认 "aces"；"agx" 柔和保留高光色彩；"reinhard" 经典压缩；"none" 关闭。 */
  mode?: "none" | "aces" | "agx" | "reinhard";
  /** 曝光倍率，默认 1。 */
  exposure?: number;
  /** 饱和度倍率，默认 1。 */
  saturation?: number;
}

/** 3D 屏幕空间环境光遮蔽（SSAO，增强物体与地面、角落的接触暗角）。 */
export interface SsaoNode {
  /** 写 `ssao` 时默认 true。 */
  enabled?: boolean;
  /** 遮蔽强度（0–2），默认 0.65。 */
  intensity?: number;
  /** 采样半径（屏幕 UV 比例），默认 0.022。 */
  radius?: number;
  /** 深度偏移（防平面自遮蔽），默认 0.0004。 */
  bias?: number;
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
 * 只有 `data` 里的格子变了时只重建改到的那几条（见 `chunk`）；别的字段变了整张地图重建，不要每帧改。
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
  /** 视差，加在每个瓦片上（同精灵的 `parallax`）；碰撞体不跟着挪，别和 `solid` 一起用。 */
  parallax?: number | [number, number];
  /** 单向平台的帧号：从下面跳得上去，站得住，角色按「下」加跳穿下去（Rust 里 `Physics::drop_through`，TSX 里 `dropThrough` command）。同一行连着的并成一块。见 docs/gameplay2d.md。 */
  oneWay?: number[];
  /** 斜坡的帧号：顶边左右两头的高（格高的 0 … 1），45° 上坡是 `{ left: 0, right: 1 }`，22.5° 两格一组 `0 → 0.5`、`0.5 → 1`。 */
  slopes?: { frame: number; left: number; right: number }[];
  /** 动画瓦片：这个帧号的格子按 `frames` 循环换帧（水、火把），只换这些格子，不重建地图。 */
  animated?: { frame: number; frames: number[]; fps: number }[];
  /** 每种瓦片自己的摩擦、弹性（比如冰面），不写的用地图的。 */
  materials?: { frame: number; friction?: number; restitution?: number }[];
  /** 点得到：`tap` 事件带上点中的格子 `cell: [列, 行]` 和那格的帧号 `frame`。 */
  tap?: boolean;
  /** 块的边长（格），默认 32，夹到 4 … 256：画的时候只画视野里的块，改格子只重建那一块所在的一条（`chunk` 行高、整张地图宽）的碰撞体。见 docs/big-scene-perf.md。 */
  chunk?: number;
}

/** `scene.perf` 写成对象时：面板、曲线、每半秒一次的 `perf` 事件各自开关。 */
export interface PerfNode {
  panel?: boolean;
  graph?: boolean;
  events?: boolean;
}

/** `perf` 事件：每半秒的汇总（开了 `perf.events` 时）。裁剪的统计是 `[画了, 一共]`。 */
export interface PerfEvent {
  fps: number;
  frameMs: number;
  worstMs: number;
  updateMs: number;
  renderMs: number;
  waitMs: number;
  /** 设备不支持时间戳、还没读回时是 null。 */
  gpuMs: number | null;
  gpuSupported: boolean;
  /** pass 太多、后面的没量：`gpuMs` 是下限。 */
  gpuTruncated: boolean;
  draws: number;
  instances: number;
  entities: number;
  particles: number;
  chunks: [number, number];
  tiles: number;
  sprites: [number, number];
  meshes: [number, number];
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
  /** 3D 粒子发射器。 */
  emitters3d?: Emitter3dNode[];
  /** 3D 贴地光环与贴花（魔法阵、技能圈、脚底投影）。 */
  decals3d?: Decal3dNode[];
  camera3d?: Camera3dNode;
  lights?: LightNode[];
  ambient?: { color?: string; brightness?: number };
  sky?: SkyNode;
  fog?: FogNode;
  shadows?: ShadowsNode;
  tonemap?: ToneMapNode;
  ssao?: SsaoNode;
  /** 重力（point / s²），默认 [0, -981]。 */
  physics?: { gravity?: [number, number] };
  /** 3D 重力，默认 [0, -981, 0]。 */
  physics3d?: { gravity?: [number, number, number] };
  /** 3D 导航网格：从地面模型与静态碰撞体自动构建可行走导航网格。 */
  navmesh3d?: NavMesh3dNode;
  /** 3D 屏幕拾取：鼠标/触控点击自动打物理射线并发 `pick3d` 事件（可写 `true` 或过滤对象）。 */
  pick3d?: boolean | Pick3dNode;
  /** 3D 区域触发器（球形/盒形体积感应，进出自动派发 `zone3d` 事件）。 */
  zones3d?: Zone3dNode[];
  joints?: JointNode[];
  /** 3D 关节。 */
  joints3d?: Joint3dNode[];
  bars?: BarNode[];
  bubbles?: BubbleNode[];
  /** 自己写的着色器：名字 → WGSL。精灵着色器写 `fn effect(e: Effect) -> vec4f`，后处理写 `fn post(p: Post) -> vec4f`。见 docs/effects2d.md。 */
  shaders?: Record<string, string>;
  /** 自己写的 3D 材质着色器：名字 → WGSL（定义 `fn shade3d(s: Surface) -> vec4f`，在 PBR 光照/阴影/雾之后、色调映射之前介入着色）。 */
  shaders3d?: Record<string, string>;
  lighting?: LightingNode;
  /** 整帧画完以后按顺序过一遍的后处理。 */
  post?: PostNode[];
  audio?: AudioNode;
  /** 时间缩放，默认 1；0 暂停（物理、补间、动画、粒子、相机平滑都停），0.5 慢一半。声音不受影响。 */
  timeScale?: number;
  /** 左上角的性能面板：帧率、更新 / 渲染 / 等待 / GPU 各花多少毫秒、draw call、实例数、实体数、裁剪统计，每半秒更新。见 docs/perf-overlay.md。
   *  写成对象还能开曲线（`graph`）、`perf` 事件（`events`），见 docs/big-scene-perf.md。 */
  perf?: boolean | PerfNode;
  /** 关卡：换了值（字符串或数字）就把场景整个重建，同 id 的节点也不留上一关的状态；Rust 收到 `LevelStart`。见 docs/levels.md。 */
  level?: string | number;
  /** 联网：写了就连中继（见 docs/networking.md 和 `netRoom()`）。 */
  net?: NetNode;
  /** 预制体：名字 → 节点组，`instances` 里按名字用（见 docs/scene-flow.md）。 */
  prefabs?: Record<string, Prefab>;
  /** 预制体实例：`prefab` 写名字或 `asset:/` 的 JSON 文件。 */
  instances?: PrefabInstance[];
  /** 换关过场：`level` 换了值时盖上、预加载新关卡、再揭开，过场里挡住输入。 */
  transition?: TransitionNode;
  /** 按纹理名或 `asset:/` 路径设置贴图（比如像素风关掉 mipmap）。 */
  textures?: Record<string, TextureOptions>;
}

/** 混音分组上的效果器（见 docs/audio-3d.md）。没写的参数用默认，超出范围的截回去。 */
export type AudioEffect =
  | { type: "lowpass"; cutoff: number; q?: number }
  | { type: "highpass"; cutoff: number; q?: number }
  | { type: "reverb"; room?: number; damping?: number; mix?: number }
  | { type: "delay"; time?: number; feedback?: number; mix?: number }
  | { type: "compressor"; threshold?: number; ratio?: number; attack?: number; release?: number; makeup?: number }
  | { type: "limiter"; ceiling?: number; release?: number };

/** 锥形指向（度，整个锥的张角）。不写 `direction` 时用跟着的模型的朝向（模型朝 -z）。 */
export interface SoundCone {
  inner: number;
  outer: number;
  outerGain?: number;
  direction?: [number, number, number];
}

/** 声源和 `sound` 命令共用的空间与衰减字段（见 docs/audio-3d.md）。 */
export interface SoundSpace {
  /** 默认 "2d"。"3d" 时按 3D 位置和听者朝向算衰减与声像。 */
  space?: "2d" | "3d";
  z?: number;
  /** 跟着这个 id 的精灵（2D）或模型（3D）走；这时 x / y / z 是相对它的偏移。 */
  follow?: string;
  /** 跟着的实体没了以后："stop"（默认）淡出停掉，"finish" 留在最后的位置播完。 */
  onLost?: "stop" | "finish";
  /** 3D 衰减模型，默认 "inverse"。 */
  rolloff?: "linear" | "inverse" | "exponential";
  /** 3D 最近距离（以内不衰减），默认 100。 */
  min?: number;
  /** 3D 最远距离（以外静音），默认 2000。 */
  max?: number;
  /** 3D 衰减系数，默认 1。 */
  factor?: number;
  cone?: SoundCone;
  /** 多普勒系数：0 关（默认），1 真实，大于 1 夸张。 */
  doppler?: number;
  /** 开启墙体遮挡（被固定刚体挡住时压低音量并加低通）。 */
  occlusion?: boolean;
}

/** 声明式的声源：场景里有就响，没了就淡出停掉。 */
export interface SoundEmitterNode extends SoundSpace {
  id: string;
  src: string;
  x?: number;
  y?: number;
  /** 默认 true；false 时出现播一次。 */
  loop?: boolean;
  /** 默认 1。 */
  volume?: number;
  /** 默认 1。 */
  pitch?: number;
  /** 默认 "sfx"。 */
  bus?: "music" | "sfx" | "ui";
  /** 出现、消失时淡入淡出的秒数，默认 0.3。 */
  fade?: number;
}

/** 听的人放在哪个精灵（2D）或模型（3D）上；不写时在相机上。 */
export interface SoundListenerNode {
  follow: string;
  space?: "2d" | "3d";
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
  /** 混音分组：背景音乐进 music，`sound` 默认进 sfx。变了才设，0.3 秒过渡过去（见 docs/audio-more.md、docs/audio-3d.md）。 */
  buses?: { music?: BusNode; sfx?: BusNode; ui?: BusNode };
  /** 带位置的声音多远就听不见，默认 1000。听的人在 2D 相机的位置。 */
  range?: number;
  /** 声明式的声源：按 id 增量同步，支持 3D、跟随实体、多普勒与遮挡。 */
  emitters?: SoundEmitterNode[];
  /** 听者跟随的精灵 / 模型；不写时 2D 在 2D 相机、3D 在 3D 相机。 */
  listener?: SoundListenerNode;
  /** 多普勒用的声速（point / 秒），默认 34300。 */
  speedOfSound?: number;
  /** 公用混响总线上的效果器链；不写时默认一个全湿混响。 */
  sendEffects?: AudioEffect[];
}

/** 一个混音分组的设置。 */
export interface BusNode {
  /** 默认 1。 */
  volume?: number;
  /** 低通滤波的截止频率（赫兹，20 到 20000），不写是关：水下、暂停时的闷声。 */
  lowpass?: number;
  /** 混响的湿声比例（0 到 1），默认 0。 */
  reverb?: number;
  /** 效果器链（最多 8 个），接在上面的 lowpass、reverb 后面按顺序处理。 */
  effects?: AudioEffect[];
  /** 往公用混响总线送多少（0 到 1），默认 0。 */
  send?: number;
  /** 这个分组的设置变了时过渡多久（秒），默认 0.3。 */
  fade?: number;
}

/** `scene.pick3d`：3D 屏幕拾取配置。 */
export interface Pick3dNode {
  /** 是否启用，默认 true。 */
  enabled?: boolean;
  /** 射线最大距离，默认 4000。 */
  maxDistance?: number;
  /** 排除的 3D 模型 id（如玩家自身，防止挡住射线）。 */
  exclude?: string;
  /** 是否也检测 sensor 碰撞体，默认 false。 */
  sensors?: boolean;
  /** 只检测这些碰撞分组（0…31）。 */
  only?: number[];
}

/** `pick3d` 事件：屏幕点击射线命中的 3D 模型与精确世界坐标。 */
export interface Pick3d {
  id: string | null;
  point: [number, number, number];
  normal: [number, number, number];
  distance: number;
  screen: [number, number];
}

/** `scene.zones3d`：3D 球形或盒形区域触发器。 */
export interface Zone3dNode {
  id: string;
  /** 形状："sphere"（默认）| "box"。 */
  shape?: "sphere" | "box";
  x?: number;
  y?: number;
  z?: number;
  /** 球形触发区半径（默认 80）。 */
  r?: number;
  /** 盒形触发区全尺寸 `[w, h, d]`（默认 `[100, 100, 100]`）。 */
  size?: [number, number, number];
  /** 仅监听这些 3D 模型 id；不写时监听所有已声明模型。 */
  watch?: string[];
  /** 是否用 3D 线框显示触发区边界（默认 false）。 */
  debug?: boolean;
}

/** `zone3d` 事件：3D 模型进入或离开触发区。 */
export interface Zone3dEvent {
  zone: string;
  model: string;
  entered: boolean;
}

/** 点中了可点击的精灵或模型。x / y 是指针的 2D 世界坐标；点中模型时 point 是 3D 命中点。 */
export interface Tap {
  id: string;
  x: number;
  y: number;
  point?: [number, number, number];
  /** 点中瓦片地图（开了 `tap`）的哪一格：`[列, 行]`。 */
  cell?: [number, number];
  /** 那一格的帧号（空格是负数）。 */
  frame?: number;
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

/** 2D 相机（不转）：中心对准 (x, y)，`zoom` 2 是放大两倍。 */
export interface CameraView {
  x: number;
  y: number;
  zoom: number;
}

/** 和引擎的 zoom 下限一样，免得除零。 */
const MIN_CAMERA_ZOOM = 1e-4;

/**
 * 游戏视图里的点（左上角原点，y 朝下，point）换成世界坐标（y 朝上）；`size` 是游戏视图的宽高。
 * 和引擎 `Camera2d::screen_to_world` 一样（不算视差、震屏），编辑器拾取用。
 */
export function screenToWorld(camera: CameraView, size: [number, number], [sx, sy]: [number, number]): [number, number] {
  const zoom = Math.max(camera.zoom, MIN_CAMERA_ZOOM);
  return [camera.x + (sx - size[0] / 2) / zoom, camera.y - (sy - size[1] / 2) / zoom];
}

/** `screenToWorld` 反过来。 */
export function worldToScreen(camera: CameraView, size: [number, number], [wx, wy]: [number, number]): [number, number] {
  const zoom = Math.max(camera.zoom, MIN_CAMERA_ZOOM);
  return [(wx - camera.x) * zoom + size[0] / 2, size[1] / 2 - (wy - camera.y) * zoom];
}

/** 炸一团粒子。`game` 是 `<GameView ref>` 拿到的节点。 */
export function burst(game: NativeComponentRef | undefined, args: Burst) {
  game?.command("burst", args);
}

/** 两个精灵节点开始（started）或停止接触；其中一个的 body 开了 collide 才发。 */
export interface Collide {
  a: string;
  b: string;
  started: boolean;
}

/** 给精灵 `id` 的刚体加上 (x, y) 的速度（point / s，和质量无关）。`id` 是有 3D 刚体的模型时加 (x, y, z)。 */
export function push(game: NativeComponentRef | undefined, args: { id: string; x: number; y: number; z?: number }) {
  game?.command("push", args);
}

/** 状态机的参数值：数是 float，true / false 是 bool。 */
export type AnimParams = Record<string, number | boolean>;

/** 一个转换。 */
export interface AnimTransition {
  /** 目标状态。 */
  to: string;
  /** 条件，全部成立才转：`"grounded"`（bool 为真或 trigger 被设了）、`"!grounded"`、`"speed > 10"`（`>` `>=` `<` `<=` `==` `!=`，右边是数）。 */
  when?: string | string[];
  /** 过渡秒数，默认 0 硬切。 */
  fade?: number;
  /** 退出时间（归一化，1 是播完一遍）：进入以后播够这么多才转。 */
  exit?: number;
}

/** 混合树：`by` 一个参数是一维（`at` 写数），两个是二维（`at` 写 [x, y]）。子片段共用一个归一化相位。 */
export interface AnimBlend {
  by: string | [string, string];
  clips: { clip: string; at: number | [number, number] }[];
}

/** 一个状态：`clip` 和 `blend` 写一个。 */
export interface AnimStateDef {
  clip?: string;
  blend?: AnimBlend;
  /** 默认 1；负数倒放（从尾巴开始）。 */
  speed?: number;
  /** 速度再乘上这个 float 参数。 */
  speedParam?: string;
  /** 默认 true。 */
  loop?: boolean;
  to?: AnimTransition[];
}

/** 一层（第 0 层是顶层的 `start` / `states` / `any`）。 */
export interface AnimLayerDef {
  name?: string;
  /** 0–1 的数，或 float 参数名；默认 1。 */
  weight?: number | string;
  /** 默认 "override"；"additive" 以参考姿势为基准叠差值（只对 3D 有用）。 */
  mode?: "override" | "additive";
  /** 骨骼遮罩：节点名，含子孙（只对 3D 有用）。 */
  mask?: string[];
  /** 叠加层的参考姿势：这个片段的 0 秒；不写用每个片段自己的 0 秒。 */
  reference?: string;
  /** 有好几个状态时要写。 */
  start?: string;
  states: Record<string, AnimStateDef>;
  /** 任意状态的转换：先于状态自己的看，转到现在的状态的跳过。 */
  any?: AnimTransition[];
}

/** 片段的事件和 2D 的帧。 */
export interface AnimClipDef {
  /** 2D：依次播的图集格子。 */
  frames?: number[];
  /** 2D，默认 10。 */
  fps?: number;
  /** 片段播到 `t` 秒（片段自己的时间）时发 animEvent。 */
  events?: { t: number; name: string }[];
}

/** 动画状态机的定义（见 docs/anim-graph.md）。 */
export interface AnimGraphDef {
  /** 参数和初始值：数是 float，true / false 是 bool，"trigger" 是 trigger。 */
  params?: Record<string, number | boolean | "trigger">;
  clips?: Record<string, AnimClipDef>;
  start?: string;
  states?: Record<string, AnimStateDef>;
  any?: AnimTransition[];
  /** 第 1 层起（顶层是第 0 层）。 */
  layers?: AnimLayerDef[];
}

/** 动画事件：权重最大的状态里权重最大的片段播到了事件 `name`。 */
export interface AnimEvent {
  id: string;
  layer: string;
  state: string;
  clip: string;
  name: string;
}

/** 往某个节点的行为树黑板上写，`value` 是 `null` 时删掉（见 docs/gameplay2d.md）。 */
export function blackboard(game: NativeComponentRef | undefined, args: { id: string; key: string; value: BlackboardValue | null }) {
  game?.command("blackboard", args);
}

/** 让站在单向平台上的精灵 `id`（角色或动态刚体）从平台上穿下去；结果用 `dropThrough` 事件回：`ok` 是穿没穿（脚下没有单向平台、还踩着实心的时不穿）。见 docs/gameplay2d.md。 */
export function dropThrough(game: NativeComponentRef | undefined, args: { id: string }) {
  game?.command("dropThrough", args);
}

/** 改瓦片地图 `id` 的格子：`tiles` 是一串 `[列, 行, 帧号]`，-1 挖空。只重建改到的块，不用重发整张地图；
 *  改到下一次声明里这一格变了、或者地图的规则变了（整张重建）为止。见 docs/big-scene-perf.md。 */
export function setTiles(game: NativeComponentRef | undefined, args: { id: string; tiles: [number, number, number][] }) {
  game?.command("setTiles", args);
}

/** `dropThrough` 的结果。 */
export interface DropThroughEvent {
  id: string;
  ok: boolean;
}

/** 设节点 `id` 的状态机参数、发 trigger（一个名字或数组）。 */
export function animSet(game: NativeComponentRef | undefined, args: { id: string; params?: AnimParams; trigger?: string | string[] }) {
  game?.command("animSet", args);
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
export function tween(game: NativeComponentRef | undefined, args: Tween) {
  game?.command("tween", args);
}

/** 震屏：幅度 `strength`（屏幕 point，默认 8）在 `duration` 秒（默认 0.3）里衰减到 0。 */
export interface Shake {
  strength?: number;
  duration?: number;
}

export function shake(game: NativeComponentRef | undefined, args: Shake = {}) {
  game?.command("shake", args);
}

/** 3D 过场运镜的关键帧。 */
export interface Camera3dKeyframe {
  pos: [number, number, number];
  target: [number, number, number];
  /** 过渡秒数（首帧写 0 表示直接切入，>0 表示从当前相机平滑过渡）。 */
  duration?: number;
  /** 可选垂直视角（度）。 */
  fov?: number;
  ease?: "linear" | "easeIn" | "easeOut" | "easeInOut";
}

/** `camera3dTrack` 命令参数：播放一段 3D 运镜轨道或提前停止。 */
export interface Camera3dTrack {
  keyframes?: Camera3dKeyframe[];
  tag?: string;
  /** 播完后平滑归位到玩法相机的秒数（默认 0.6）。 */
  returnDuration?: number;
  stop?: boolean;
}

export interface Camera3dTrackEnd {
  tag: string | null;
  cancelled: boolean;
}

/** 播放 3D 过场运镜轨道（结束后自动平滑归位到当前 3D 相机）。 */
export function camera3dTrack(game: NativeComponentRef | undefined, args: Camera3dTrack) {
  game?.command("camera3dTrack", args);
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

/**
 * 播一次音效。`pitch` 是播放速度（2 = 高八度，限制在 0.1 到 8），`pan` 从 -1（左）到 1（右）。同时发声数有上限，超出的会被丢掉。
 * `bus` 默认 "sfx"。`x`、`y` 都写了就按位置播：离相机越远越小，在左边就偏左。
 */
export function sound(
  game: NativeComponentRef | undefined,
  args: { src: string; volume?: number; pitch?: number; pan?: number; bus?: "music" | "sfx" | "ui"; x?: number; y?: number } & SoundSpace,
) {
  game?.command("sound", args);
}

/** 飘一行字。`game` 是 `<GameView ref>` 拿到的节点。 */
export function floatText(game: NativeComponentRef | undefined, args: FloatText) {
  game?.command("floatText", args);
}

/** 手柄震动：第 `player` 个玩家（不填是第一个）的手柄，`low` 低频（重）马达、`high` 高频（轻）马达的强度 0 … 1，`duration` 秒（不填 0.2，最多 5）。新的一次顶掉这个手柄上正在震的。 */
export interface RumbleArgs {
  player?: number;
  low?: number;
  high?: number;
  duration?: number;
}

/** 让手柄震一下（见 docs/input-actions.md）；没有手柄、手柄不会震时什么都不发生。 */
export function rumble(game: NativeComponentRef | undefined, args: RumbleArgs) {
  game?.command("rumble", args);
}

/** 停掉第 `player` 个玩家（不填是第一个）的手柄上正在震的。游戏暂停、窗口失焦、切到后台时引擎和平台层会自己停。 */
export function rumbleStop(game: NativeComponentRef | undefined, player = 0) {
  game?.command("rumble", { player, stop: true });
}

/** 发一次按键给引擎，和键盘、手柄一样进 `Input`（见 docs/touch-controls.md）。改键以后键名对应的动作会变，触屏按钮用 `<ActionButton>`。 */
export function key(game: NativeComponentRef | undefined, code: string, down: boolean) {
  game?.command("key", { code, down, repeat: false });
}

/** 按住发按键的屏幕按钮（testID 是 `key-<code>`）。用触摸而不是 press：同时按住两个按钮（跑着跳）要靠各自的手指。 */
export function KeyButton(props: { game: () => NativeComponentRef | undefined; code: string; label: string; class?: string }) {
  const held = heldKey((down) => key(props.game(), props.code, down));
  const ids = (e: GestureResponderEvent) => e.changedTouches.map((t) => t.identifier);
  return (
    <View
      testID={`key-${props.code}`}
      class={`w-16 h-16 rounded-full bg-white/20 items-center justify-center active:bg-white/40 ${props.class ?? ""}`}
      onTouchStart={(e: GestureResponderEvent) => held.start(ids(e))}
      onTouchEnd={(e: GestureResponderEvent) => held.end(ids(e))}
      onTouchCancel={(e: GestureResponderEvent) => held.end(ids(e))}
    >
      <Text class="text-white text-2xl">{props.label}</Text>
    </View>
  );
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
  /** 动作的绑定表（`inputMap().prop()`，见 docs/input-actions.md）：引擎按它算动作的状态，Rust 用 `Actions` 问，TSX 收 `action` 事件。 */
  actions: ActionMap;
  /** Android：锁住屏幕方向（跟着传感器转 180°），游戏视图没了时放开。不填时跟着系统转。别的平台不管。 */
  screenOrientation: "landscape" | "portrait";
}

interface BuiltinEvents {
  tap: Tap;
  collide: Collide;
  animEnd: AnimEnd;
  /** 动画状态机的事件（见 docs/anim-graph.md）。 */
  animEvent: AnimEvent;
  tweenEnd: TweenEnd;
  key: KeyInput;
  /** 动作变了：按钮按下 / 松开，轴的方向变了。 */
  action: ActionEvent;
  /** 手柄接入、拔出、换了玩家。 */
  pad: PadEvent;
  /** 换关（场景的 `level` 换了值）后新关卡的实体建好了：要对新关卡发的命令（补间等）在这里发。 */
  levelStart: { level: string };
  chase: ChaseEvent;
  /** 行为树里的 `emit`（见 docs/gameplay2d.md）。 */
  behavior: BehaviorEvent;
  /** 气泡的字打完了。 */
  typed: { id: string };
  fps: FrameStats;
  error: EngineError;
  /** 引擎画出了第一帧（web 上资源取完、GPU 好了）。 */
  ready: {};
  /** 联网的状态、成员进出、消息（`scene.net`，见 `netRoom()`）。 */
  net: NetEvent;
  /** HTTP 请求的结果（见 `httpClient()`）。 */
  http: HttpReply;
  /** 3D 空间查询的结果（见 `queries3d()`）。 */
  query3d: Query3dReply;
  /** 3D 屏幕拾取命中事件（`scene.pick3d`）。 */
  pick3d: Pick3d;
  /** 3D 过场运镜轨道结束或被取消。 */
  camera3dTrackEnd: Camera3dTrackEnd;
  /** 3D 区域触发器进出事件（`scene.zones3d`）。 */
  zone3d: Zone3dEvent;
  dropThrough: DropThroughEvent;
  /** 性能汇总（`scene.perf.events`）。 */
  perf: PerfEvent;
  /** 预加载的进度（见 `preloader()`）。 */
  preload: PreloadEvent;
  /** 换关过场的阶段和加载进度（见 `transitions()`）。 */
  transition: TransitionEvent;
}

type TouchPhase = "start" | "move" | "end" | "cancel";

export type GameViewProps<P, E> = NativeComponentProps<Partial<BuiltinProps> & P, BuiltinEvents & E>;

/**
 * 生成游戏视图组件。`props` / `events` 是游戏自己的通道名（Rust 侧 `add_prop` / `add_state` / `add_event`），
 * 类型由泛型给出；内置 prop `scene` / `actions`、事件 `tap` / `collide` / `animEnd` / `animEvent` / `tweenEnd` / `key` / `action` / `pad` / `levelStart` / `chase` / `behavior` / `typed` / `fps` / `error` / `ready` / `net` / `http` / `query3d` / `pick3d` / `camera3dTrackEnd` / `zone3d` / `dropThrough` / `preload` / `transition`，触摸自动转成引擎的 `touch` command。
 */
export function gameView<P extends object = {}, E extends object = {}>(
  spec: { props?: readonly (keyof P & string)[]; events?: readonly (keyof E & string)[] } = {},
) {
  const Native = nativeComponent<BuiltinProps & P, BuiltinEvents & E>("nge.game", {
    props: [...new Set(["scene", "captureKeys", "actions", "screenOrientation", ...(spec.props ?? [])])] as (keyof (BuiltinProps & P) & string)[],
    events: [...new Set(["tap", "collide", "animEnd", "animEvent", "tweenEnd", "key", "action", "pad", "levelStart", "chase", "behavior", "typed", "fps", "error", "ready", "net", "http", "query3d", "pick3d", "camera3dTrackEnd", "zone3d", "dropThrough", "preload", "transition", ...(spec.events ?? [])])] as (keyof (BuiltinEvents & E) & string)[],
  });

  return (props: GameViewProps<P, E>) => {
    let node: NativeComponentRef | undefined;
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
        ref={(n: NativeComponentRef) => {
          node = n;
          (user.ref as ((n: NativeComponentRef) => void) | undefined)?.(n);
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

// 本地化见 docs/localization.md
export { i18n, missing, plural, resolveLocale, type I18n, type I18nOptions, type Message, type Messages, type Params, type PluralCategory } from "./i18n";
// 联网见 docs/networking.md
export { ARENA_KEY, netRoom, netScore, netSend, relayHttp, remoteId, type ArenaLayout, type NetEvent, type NetNode, type NetPeer, type NetRoom, type NetStats, type NetStatus } from "./net";
export { httpClient, json, type HttpClient, type HttpOptions, type HttpReply, type HttpResult } from "./http";
export { preloader, transitions, type LoadFailure, type Loading, type Prefab, type PrefabInstance, type PreloadEvent, type Preloader, type TextureOptions, type TransitionEvent, type TransitionNode, type Transitions } from "./flow";
export { queries3d, type Hit3d, type Queries3d, type Query3dOptions, type Query3dReply, type QueryShape3d } from "./physics3d";
// 输入映射见 docs/input-actions.md
export { bindingsOf, conflictsOf, inputMap, KEY_NAMES, keyAt, slotsOf, withKey, type ActionEvent, type ActionMap, type AnalogAxis, type AnalogStick, type Binding, type Bindings, type InputMap, type InputMapOptions, type InputStore, type PadEvent, type Part, type PendingBind, type Slot } from "./input";
// 触屏控制见 docs/touch-input.md
export { act, ActionButton, actionButtonId, cooldownFill, heldKey, pressedLook, repeater, touchGestures, VirtualStick } from "./touch";
export { knobOffset, snapDirection, STICK_DEFAULTS, STICK_REACH, stickCommand, stickKeep, stickOutput, stickStep, stickValue, type Directions, type StickFinger, type StickShape, type StickState, type StickTouch } from "./stick";
export { GESTURE_DEFAULTS, GESTURE_IDLE, gestureHandlers, gestureStep, type Gesture, type GestureInput, type GestureOptions, type GestureState, type GestureTimers, type SwipeDirection } from "./gestures";
// 存档槽见 docs/saves.md
export { saveSlots, type LoadResult, type SaveSlots, type SaveSlotsOptions, type SaveStore, type SlotInfo } from "./saves";

/**
 * 存在设备上的 signal（nsh 的 `Storage`，键名是 `nge:<key>`）：一开始是 `initial`，读到存档后换成存的值；
 * `set` / `update` 时一起写进存档。读到存档之前就 `set` 过的，以 `set` 的为准（存档整个被换掉，读档只要几毫秒，一般碰不到）。
 * 存的值结构不对（以前的版本存的）时这里不管，读的地方自己补齐。同一个 key 只调一次，调两次是两个互不同步的 signal。见 docs/levels.md。
 */
export function saved<T>(key: string, initial: T): Signal<T> {
  const value = signal<T>(initial);
  const storageKey = `nge:${key}`;
  let changed = false;
  Storage.getItem(storageKey).then(
    (raw) => {
      if (raw === null || changed) return;
      try {
        value.set(JSON.parse(raw) as T);
      } catch {
        console.warn(`存档 ${storageKey} 不是合法的 JSON，用初始值`);
      }
    },
    (e) => console.warn(`读存档 ${storageKey} 失败: ${e}`),
  );
  const set = (next: T) => {
    changed = true;
    value.set(next);
    Storage.setItem(storageKey, JSON.stringify(next)).catch((e) => console.warn(`写存档 ${storageKey} 失败: ${e}`));
  };
  return Object.assign(() => value(), { set, update: (fn: (v: T) => T) => set(fn(untracked(value))) }) as Signal<T>;
}
