// nge 的 TSX 绑定：<GameView> 组件和场景类型（和 crates/nge-scene 的协议一一对应）。
import { nativeComponent, signal, Storage, Text, untracked, View, type GestureResponderEvent, type NativeComponentProps, type NativeComponentRef, type Signal } from "nsh";

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
 * 后处理的一遍：`shader` 是 `scene.shaders` 或内置的后处理（grayscale、vignette、pixelate），
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
  /** 初速度（point / s）。 */
  velocity?: [number, number];
  /** 初角速度（度 / s）。 */
  angularVelocity?: number;
  /** 碰撞开始 / 结束时发 collide 事件。 */
  collide?: boolean;
  /** 不让碰撞把它转起来（平台游戏的角色）。 */
  fixedRotation?: boolean;
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
  /** 跟着这个精灵（id）画的位置走，x / y 变成相对它的偏移；找不到就藏起来。 */
  follow?: string;
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
  /** 跟着这个精灵走，x / y 是相对它的偏移；位置是气泡的底边中点。 */
  follow?: string;
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
  /** 播模型里的一段骨骼 / 节点动画（glTF 的 animations），见 docs/3d-more.md。只改 speed、loop 时不从头播。 */
  animation?: ModelAnimation;
  /** 3D 刚体（见 docs/physics3d.md）。有了它位置和旋转归物理管：声明的位置、角度、缩放没变时不拉回去；`attach`、`spin` 不生效。 */
  body?: ModelBody;
  /** 拖尾（见 docs/particles3d.md）：模型走过的地方留一条朝着相机的带子。去掉时马上消失；只改参数时保留已经记下的点。 */
  trail?: TrailNode;
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
}

/** 模型的 3D 刚体和碰撞体。和 2D 刚体在两个世界里，互不碰撞。 */
export interface ModelBody {
  /** 默认 "dynamic"。kinematic 跟着声明的位置平滑移动，会推开别的刚体。 */
  kind?: "dynamic" | "fixed" | "kinematic";
  /** 默认 "box"。capsule 是竖着的胶囊。 */
  shape?: "box" | "ball" | "capsule";
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
  /** 投影子：方向光、聚光灯、点光源各只算第一盏开着的，见 docs/3d-more.md、docs/render3d-more.md。 */
  shadows?: boolean;
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
  /** 视差，加在每个瓦片上（同精灵的 `parallax`）；碰撞体不跟着挪，别和 `solid` 一起用。 */
  parallax?: number | [number, number];
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
  camera3d?: Camera3dNode;
  lights?: LightNode[];
  ambient?: { color?: string; brightness?: number };
  /** 重力（point / s²），默认 [0, -981]。 */
  physics?: { gravity?: [number, number] };
  /** 3D 重力，默认 [0, -981, 0]。 */
  physics3d?: { gravity?: [number, number, number] };
  joints?: JointNode[];
  bars?: BarNode[];
  bubbles?: BubbleNode[];
  /** 自己写的着色器：名字 → WGSL。精灵着色器写 `fn effect(e: Effect) -> vec4f`，后处理写 `fn post(p: Post) -> vec4f`。见 docs/effects2d.md。 */
  shaders?: Record<string, string>;
  lighting?: LightingNode;
  /** 整帧画完以后按顺序过一遍的后处理。 */
  post?: PostNode[];
  audio?: AudioNode;
  /** 时间缩放，默认 1；0 暂停（物理、补间、动画、粒子、相机平滑都停），0.5 慢一半。声音不受影响。 */
  timeScale?: number;
  /** 左上角的性能面板：帧率、更新 / 渲染 / 等待各花多少毫秒、draw call、实例数、实体数，每半秒更新。见 docs/perf-overlay.md。 */
  perf?: boolean;
  /** 关卡：换了值（字符串或数字）就把场景整个重建，同 id 的节点也不留上一关的状态；Rust 收到 `LevelStart`。见 docs/levels.md。 */
  level?: string | number;
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
  /** 混音分组：背景音乐进 music，`sound` 默认进 sfx。变了才设，0.3 秒过渡过去（见 docs/audio-more.md）。 */
  buses?: { music?: BusNode; sfx?: BusNode; ui?: BusNode };
  /** 带位置的声音多远就听不见，默认 1000。听的人在 2D 相机的位置。 */
  range?: number;
}

/** 一个混音分组的设置。 */
export interface BusNode {
  /** 默认 1。 */
  volume?: number;
  /** 低通滤波的截止频率（赫兹，20 到 20000），不写是关：水下、暂停时的闷声。 */
  lowpass?: number;
  /** 混响的湿声比例（0 到 1），默认 0。 */
  reverb?: number;
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
  args: { src: string; volume?: number; pitch?: number; pan?: number; bus?: "music" | "sfx" | "ui"; x?: number; y?: number },
) {
  game?.command("sound", args);
}

/** 飘一行字。`game` 是 `<GameView ref>` 拿到的节点。 */
export function floatText(game: NativeComponentRef | undefined, args: FloatText) {
  game?.command("floatText", args);
}

/** 发一次按键给引擎，和键盘、手柄一样进 `Input`（见 docs/touch-controls.md）。 */
export function key(game: NativeComponentRef | undefined, code: string, down: boolean) {
  game?.command("key", { code, down, repeat: false });
}

/** 一个屏幕按钮上按着的手指：第一根按下时 `send(true)`，最后一根松开时 `send(false)`；没按着时的松开不发。 */
export function heldKey(send: (down: boolean) => void) {
  const fingers = new Set<number>();
  return {
    start(ids: number[]) {
      const was = fingers.size;
      for (const id of ids) fingers.add(id);
      if (was === 0 && fingers.size > 0) send(true);
    },
    end(ids: number[]) {
      const was = fingers.size;
      for (const id of ids) fingers.delete(id);
      if (was > 0 && fingers.size === 0) send(false);
    },
  };
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
}

interface BuiltinEvents {
  tap: Tap;
  collide: Collide;
  animEnd: AnimEnd;
  tweenEnd: TweenEnd;
  key: KeyInput;
  /** 换关（场景的 `level` 换了值）后新关卡的实体建好了：要对新关卡发的命令（补间等）在这里发。 */
  levelStart: { level: string };
  chase: ChaseEvent;
  /** 气泡的字打完了。 */
  typed: { id: string };
  fps: FrameStats;
  error: EngineError;
  /** 引擎画出了第一帧（web 上资源取完、GPU 好了）。 */
  ready: {};
}

type TouchPhase = "start" | "move" | "end" | "cancel";

export type GameViewProps<P, E> = NativeComponentProps<Partial<BuiltinProps> & P, BuiltinEvents & E>;

/**
 * 生成游戏视图组件。`props` / `events` 是游戏自己的通道名（Rust 侧 `add_prop` / `add_state` / `add_event`），
 * 类型由泛型给出；内置 prop `scene`、事件 `tap` / `collide` / `animEnd` / `tweenEnd` / `key` / `levelStart` / `chase` / `typed` / `fps` / `error` / `ready`，触摸自动转成引擎的 `touch` command。
 */
export function gameView<P extends object = {}, E extends object = {}>(
  spec: { props?: readonly (keyof P & string)[]; events?: readonly (keyof E & string)[] } = {},
) {
  const Native = nativeComponent<BuiltinProps & P, BuiltinEvents & E>("nge.game", {
    props: [...new Set(["scene", "captureKeys", ...(spec.props ?? [])])] as (keyof (BuiltinProps & P) & string)[],
    events: [...new Set(["tap", "collide", "animEnd", "tweenEnd", "key", "levelStart", "chase", "typed", "fps", "error", "ready", ...(spec.events ?? [])])] as (keyof (BuiltinEvents & E) & string)[],
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
