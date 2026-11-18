# 3D 粒子补齐：发射形状、序列帧、朝向、曲线、软粒子、碰撞、子发射器、受光

> 前置：3D 粒子和拖尾（[particles3d.md](particles3d.md)）、3D 画质（[render3d-quality.md](render3d-quality.md)）、3D 物理（[physics3d.md](physics3d.md)）、性能面板（[perf-overlay.md](perf-overlay.md)）。

## 目标

3D 粒子现在只能从一个点往一个方向锥里喷，粒子是一张不变的方片，大小和颜色只在出生、死亡两头之间插值。做不出：
- 从球面、地上一圈、模型表面冒出来的粒子；
- 会动的火苗（序列帧）、拉成一道线的火星、贴着地面扩散的冲击波；
- 先变大再变小、中途变色的烟；
- 穿进地面时边上一刀切的烟（没有软粒子）；
- 落地弹起、落地溅出火花、炸开成一团的烟花（没有碰撞和子发射器）；
- 在暗处也亮着的烟（不受光）。

另外粒子每帧新建排序数组和带子的点，性能面板也看不出有多少粒子。

**验收标准**：
- 发射形状：点、球（表面 / 体积）、半球、盒子（表面 / 体积）、圆环或圆盘、锥形（带底面半径）、网格表面（按三角形面积加权）；发射方向可以选沿法线、沿方向锥、随机。
- 序列帧：贴图按 grid 切帧，按寿命播完、按固定 fps 循环、随机起始帧，可选相邻两帧混合。
- 朝向：朝着相机（原来的）、沿速度拉长（长度随速度、带系数）、水平平铺、固定的世界朝向。
- 大小、颜色、透明度、速度随寿命按多关键点曲线 / 渐变变化；速度阻尼；可选湍流（curl noise）。
- 软粒子：靠近不透明表面时淡出，软化距离可调；GL 后端没有，退成硬边。
- 碰撞：地面平面或场景碰撞体（rapier 射线），反弹（系数和摩擦）、死亡、触发子发射器；每帧查询数有上限。
- 子发射器：粒子出生、死亡、碰撞时在它的位置做一次 burst；防止无限递归，总数有上限。
- 受光（开关）：环境光（常量 + 天空 SH）和方向光的 lambert；不开的照旧。
- 性能：排序数组、顶点缓冲、带子的点每帧复用；粒子总数软上限，统计进性能面板。
- 场景 JSON 和 TSX 的 `Emitter3dNode` 都能配；新字段逐字段宽松解析。

## 做法

### 发射形状（`nge-particles` 的 `shape3d.rs`）

```rust
pub enum EmitShape {
    Point,                                  // 默认：和原来一样
    Sphere { radius: f32, surface: bool },  // surface：只在球面上，否则球体内均匀
    Hemisphere { radius: f32, surface: bool }, // 本地 +Y 那一半
    Box { size: Vec3, surface: bool },      // size 是整个长宽高；表面按面积选面
    Circle { radius: f32, inner: f32 },     // 本地 xz 平面上的圆环（inner..radius），inner = 0 是圆盘、inner = radius 是一圈线
    Cone { radius: f32, angle: f32 },       // 底面半径 radius 的圆盘上出生，越靠边越往外偏，最外圈偏 angle（弧度）
    Mesh(MeshSource),                       // 网格表面
}
pub enum EmitDirection { Cone /* 默认：原来的 direction + spread */, Normal, Random }
```

- 取点是纯函数 `sample(shape, rng, surface) -> (位置, 法线)`，本地坐标，再乘发射器的旋转（不乘缩放），加上发射器的位置。
  - 球体积：半径按 `cbrt(u)` 取（体积均匀）；圆环按 `sqrt(lerp(inner², r², u))` 取（面积均匀）；盒子表面按三对面的面积选面。
  - 法线：球、半球是从球心往外；盒子表面是那个面的朝向，体积里是从中心往外；圆环是平面里从圆心往外（冲击波往外扩）；锥形是按离中心的远近往外偏的方向；网格是三角形插值后的法线；点没有法线，用 `direction`。
  - 半径、尺寸是负数或 NaN 时当 0。
  - 网格还没加载时发射器原地等：不发射，`burst` 和攒着的 `rate` 都不消耗（等的那几秒的 `rate` 不补发），`despawn_when_done` 的也不算放完；加载完但没有三角形（或加载失败）时不再等，一个也不发（warn 一次）。
- `EmitDirection`：
  - `Cone`：原来的 `direction` 加 `spread`，和形状无关。
  - `Normal`：沿取点处的法线，再在 `spread` 的锥里散开（`spread` 为 0 就是正沿法线）。
  - `Random`：球面上均匀，不看 `spread`。
- 网格表面 `MeshSource`：
  - `Surface(Arc<MeshSurface>)`：自己给三角形；
  - `Model { model: ModelId, scale }`：从 `Assets3d` 的模型取（乘部件的 `local` 和 `scale`）；
  - `Gltf { gltf: Handle<Gltf>, scale }`：glTF 加载完后展开成模型再取（和模型节点共用同一份展开，`nge_render3d::gltf_model`）。
  - `MeshSurface` 存三角形和面积的前缀和，取点先按面积二分选三角形，再在三角形里均匀取（`sqrt` 法）；面积是 0 的三角形不会被选中。
  - 模型、glTF 的表面在 `MeshSurfaces` 资源里按（来源，缩放）缓存，一个排他系统在模拟前补齐：没有网格发射器时直接返回；没有发射器再用的来源从缓存里清掉；模型编号还不存在时不缓存成失败，之后加进来的模型下一帧就能用。
    - 资源服务器同一个 uri 只加载一次，加载失败的 glTF 不会自己重试；场景热重载换了 uri（新的句柄）就是新的来源，旧的那份被清掉。
  - 法线按 `(local⁻ᵀ · n) / scale` 再归一化（位置是先 `local` 再缩放），部件有旋转、缩放不均匀时也垂直于表面。

### 曲线和渐变（`curve.rs`）

- `Curve`：若干 `(t, 值)`，`Gradient`：若干 `(t, 线性 RGBA)`；构造时按 `t` 排序、丢掉 NaN。求值：第一点之前取第一点、最后一点之后取最后一点、中间线性插值；空的曲线没有意义，当没设。
- 发射器多了：
  - `size_curve: Option<Curve>`：设了就用它（世界单位，`t` 是年龄 / 寿命），不再用 `size` 的两头插值。
  - `color_gradient: Option<Gradient>`：设了就替换 `color` 的两头插值。
  - `alpha_curve: Option<Curve>`：乘到透明度上。
  - `speed_curve: Option<Curve>`：乘到这一帧的位移上（速度本身照常受重力、阻尼、湍流）。
- 阻尼 `drag`：每帧速度乘 `e^(-drag · dt)`，和帧率无关；负数当 0。
- 湍流 `noise: Option<Noise3d { strength, frequency, scroll }>`（默认 1、1、0）：三维值噪声做三个势场，解析求偏导再取 curl（无散度，粒子不会往一处聚），按 `strength` 加到速度上；`scroll` 让噪声场随时间沿 y 移动。
  - 纯函数 `curl(p)`，`p` 是 `位置 × frequency + (0, scroll(time, 速度), 0)`。三个势场共用格点：一个格点哈希一次出三个值，每个粒子每帧哈希 8 次。
  - 格点每 256 格重复；`scroll(time, 速度)` 用 f64 算 `time × 速度` 再按 256 取模，跑了很久也不丢小数。

### 序列帧（`sheet.rs`）

```rust
pub struct SpriteSheet { pub columns: u32, pub rows: u32, pub frames: u32 /* 0 = 全部 */, pub mode: SheetMode, pub random_start: bool, pub blend: bool }
pub enum SheetMode { Lifetime /* 按寿命播完 */, Fps(f32) /* 按固定 fps 循环 */ }
```

- 贴图从左上角按行切成 `columns × rows` 格，第 `i` 帧在第 `i / columns` 行、第 `i % columns` 列。
- 选帧是纯函数 `SpriteSheet::pick(age, lifetime, start) -> (这一帧, 下一帧, 混合)`：
  - `Lifetime`：`f = start + t × 帧数`，`t` 是年龄 / 寿命；
  - `Fps`：`f = start + 年龄 × fps`；
  - 都按帧数取模（循环）；`Lifetime` 不随机起始（`random_start` 关着）时最后一刻停在最后一帧，不绕回第一帧；随机起始的（哪怕抽到第 0 帧）照样绕回来。
  - `blend` 时混合是 `f` 的小数部分，否则是 0。
- `random_start`：出生时在 `[0, 帧数)` 里随机取 `start`。
- 方片带两组 uv（这一帧、下一帧）和混合比例，着色器采样两次再混合；混合是 0 时只用第一组。

### 朝向（`nge-render3d` 的 `Facing`）

```rust
pub enum Facing { Camera /* 默认 */, Velocity { velocity: Vec3, stretch: f32 }, Horizontal, Fixed(Quat) }
```

- `Camera`：原来的方片。
- `Velocity`：长边沿速度在屏幕上的方向，宽 `size`、长 `size + |速度| × stretch`，中心在粒子位置；宽的方向是 `速度 × 指向相机的方向`。速度是 0、或和视线平行（正对相机飞）时退成 `Camera`。`rotation` 不起作用。
- `Horizontal`：躺在 xz 平面里（法线 +Y），`rotation` 绕 y 转。
- `Fixed(q)`：本地 xy 平面的方片乘 `q`，`rotation` 绕本地 z 转。场景里写欧拉角（度），顺序和模型的 `rotation` 一样：先绕 x、再绕 y、最后绕 z（`EulerRot::YXZ`）。
- 发射器的 `facing: ParticleFacing`（`Camera`、`Velocity { stretch }`、`Horizontal`、`Fixed(Quat)`）每帧按粒子的速度填 `Facing`。

### 软粒子

- 方片多了 `soft`（软化距离，世界单位，0 不软化）：片元离后面不透明表面的距离（都按沿相机前方的距离算）小于 `soft` 时按 `距离 / soft` 淡出（乘到透明度上，叠加的也一样少加）。
- 读深度：主 pass 画完不透明和半透明网格、天空后结束，深度保存；方片另开一个 pass，深度附件只读（`depth_ops`、`stencil_ops` 都是 `None`），同一张深度纹理（只取深度那一面）同时绑成 `texture_depth_2d`，`textureLoad` 读这个像素的深度，用逆的 `view_proj` 还原成世界坐标。调试线再开一个 pass 画在最后。
  - 这一帧没有软粒子时还是一个 pass，绑一张 1 × 1 的假深度。
  - 深度纹理平时只当附件（不带 `TEXTURE_BINDING`），第一次出现软粒子时才重建成能绑成纹理的，之后一直是（不来回重建）。
  - 代价：有软粒子的帧，主 pass 结束时多存一次深度、方片的 pass 开头多读一次颜色（移动端的 tile GPU 上是多一次显存来回）。手机上的性能还没验证。
- GL 后端（WebGL2、老 Android 的 GLES）：同一张纹理既是附件又被采样是反馈回路，WebGL2 直接报错；深度格式（`Depth24PlusStencil8`）也不能拷贝。所以 GL 上不分 pass、着色器里不读深度，软粒子退成硬边，别的照常。

### 碰撞（`collide3d.rs`）

```rust
pub struct ParticleCollision {
    pub ground: Option<f32>, // 地面平面的高度（世界 y）
    pub scene: bool,         // 和场景的 3D 碰撞体碰（rapier 射线）
    pub bounce: f32,         // 法向速度反弹系数，默认 0.5
    pub friction: f32,       // 切向速度损失比例 0–1，默认 0
    pub kill: bool,          // 碰到就死
    pub radius: f32,         // 粒子半径，停在表面外这么远
    pub group: u8,           // 场景查询跳过这个碰撞分组的（同 QueryFilter::group，0 不跳过）
    pub only: u32,           // 只碰这些分组（位掩码，第 0 位是不分组的），默认全部
}
```

- 每帧粒子从 `p` 走到 `p'`：先查地面（`p.y ≥ 地面 + 半径 > p'.y` 时按比例找到碰撞点），再查场景（`Physics3d::raycast(p, p' - p, |p' - p| + 半径)`），取近的那个。
- 反弹是纯函数 `bounce(v, n, bounce, friction)`：法向分量反向乘 `bounce`，切向分量乘 `1 - friction`；位置放到碰撞点沿法线外 `radius`（至少一点点）。
- 场景查询按 `group`、`only` 过滤；出生在碰撞体里面的粒子（射线碰到的距离是 0）不被这个碰撞体挡，也不反弹，自己穿出去（`Physics3d` 的射线只给最近的一个，所以在里面时它后面的碰撞体也查不到）。
- 每帧场景查询有总上限 `ParticleBudget3d::max_queries`（默认 256，所有发射器一起算），按每个池子（包括子发射器的）这一帧开头要查的粒子数占的比例分：`上限 × 池子的粒子数 / 所有要查的粒子数`，每个有粒子的池子至少一次，所以池子很多时一帧可能超出上限几次。
  - 查不了的粒子这一帧只查地面；每个池子从上一帧停下的地方接着查（轮流），不会总是同一批粒子查不到。
- 刚碰上时发 `collision` 事件（给子发射器）；`kill` 时粒子死掉，同时发 `death` 事件。
  - 上一帧就贴着表面的（停着、贴着地面滑）只修正位置和速度，不再发 `collision`：粒子记着 `touching`，这一帧没碰到才清掉（这一帧没轮到查场景的不动它）。每弹起来一次再落下，算新的一次碰撞。
- 开了 `scene` 但没有 3D 物理（`Physics3d` 资源）时只碰地面，warn 一次。

### 子发射器

```rust
pub struct SubEmitter3d { pub on: SubTrigger /* Birth / Death / Collision */, pub count: u32, pub inherit: f32, pub emitter: ParticleEmitter3d }
```

- 发射器的 `sub_emitters: Vec<SubEmitter3d>`。每个子发射器在父粒子那个事件的位置 burst `count` 个，粒子存在父发射器的 `Particles3d` 里（一棵树：每个子发射器一个池子，子发射器的子发射器再往下挂）。
  - 子发射器的 `rate`、`burst`、`despawn_when_done` 不起作用。
  - 子发射器的池子按下标对应：调换 `sub_emitters` 的顺序，已经发出去的子粒子会按新顺序换成另一个子发射器的参数（画法、寿命照新的算）；要换顺序时最好连同发射器重建。
  - 速度 = 自己的发射速度 + 父粒子速度 × `inherit`。
  - 碰撞事件的朝向：本地 +Y 对着碰到的面的法线（`direction` 写 `[0, 1, 0]` 就是从地面溅起）；出生、死亡事件不转。
- 每帧每个池子只走一遍：先模拟自己的粒子（死亡、碰撞事件），再发新粒子（出生事件），再把这些事件交给子池子。
- 防止失控：
  - 嵌套最多 `MAX_SUB_DEPTH = 3` 层，更深的不模拟（warn 一次）。场景里子发射器是嵌套写的，写不出环。
  - 每个池子有自己的 `max`；所有 3D 粒子合起来有软上限 `ParticleBudget3d::limit`（默认 20000）：到了就不再发射（每帧开头按存活总数算剩多少），被它挡掉的数记进统计（`ParticleStats::dropped`，单个池子到了自己的 `max` 的不算），warn 一次。
  - 发射循环最多走「还能发的个数」次：`count` 再大，预算用完就停，不空转；个数相乘用饱和乘法。场景里 `count` 最多 `MAX_SUB_COUNT = 10000`，多的 warn 一次按上限算。
- 父发射器 `despawn_when_done` 时要等子池子也空了才删。

### 受光

- 方片多了 `lit`。开着时颜色 = 贴图 × 顶点颜色 × (常量环境光 + 天空 SH 辐照度 / π × 环境漫反射强度（有天空时） + Σ 方向光 × 包裹的 lambert / π)。
  - 法线：朝相机的是指向相机，`Horizontal` 是 +Y，`Fixed` 是转过的 +Z，拉长的是垂直于长边、尽量朝相机。
  - 包裹的 lambert：`((n·l) × 0.5 + 0.5)²`，背光面不是死黑（方片很薄，光会透过来）。
  - 点光源、聚光灯、阴影不照粒子。
- 不开的照旧：只有贴图 × 颜色，受雾影响。

### 性能

- `Billboards` 的方片、带子的点放在两个平的数组里（带子是点数组里的一段），`clear` 留着容量；`build` 用渲染器留着的排序数组和带子长度数组；顶点缓冲按两倍扩容、不缩。
- 排序用 `sort_unstable_by`，比较时最后按放进来的下标（方片在前、带子在后）分先后，一样远、一样贴图的照放进来的顺序画。
- 带子和方片共用一种顶点格式：带子的第二组 uv、法线、`params`（混合、软化、受光）都是 0，用不到，每个顶点白占 36 字节；带子的点很少，没再分一种格式和管线。
- 粒子池用 `retain_mut`，事件数组每个池子一份留着。
- `ParticleBudget3d { limit, max_queries }` 资源（`Particles3dPlugin` 加上）。
- `nge-ecs` 的 `ParticleStats { alive, dropped, queries }` 资源每帧由 3D 粒子系统写；性能面板第三行多一项「粒子 N」（`PerfReport::particles`）。

### 场景和 TSX

`Emitter3dNode` 多了（都可选；写错的字段 warn 一次当没写，别的照用；多余的键也 warn）：

| 字段 | 意思 |
| --- | --- |
| `shape` | `{ kind: "point" }`、`{ kind: "sphere" \| "hemisphere", radius, surface? }`、`{ kind: "box", size: [x, y, z], surface? }`、`{ kind: "circle", radius, inner? }`、`{ kind: "cone", radius, angle }`（度）、`{ kind: "mesh", model: "asset:/….glb", scale? }` |
| `emitDirection` | `"cone"`（默认）、`"normal"`、`"random"` |
| `sizeCurve`、`alphaCurve`、`speedCurve` | `[[t, 值], …]` |
| `colorGradient` | `[[t, "#rrggbbaa"], …]` |
| `drag` | 阻尼 |
| `angle` | 出生时的转角范围 `[最小, 最大]`（度），不写时一整圈随机（Rust 的 `angle: Option<(f32, f32)>`）；序列帧的火苗写 `[0, 0]` 才是正的 |
| `noise` | `{ strength?, frequency?, scroll? }`，默认 1、1、0 |
| `facing` | `"camera"`（默认）、`"velocity"`、`"horizontal"`、`[x, y, z]`（固定朝向，欧拉角度，先绕 x、再绕 y、最后绕 z，和模型的 `rotation` 一样） |
| `stretch` | 拉长系数（`facing: "velocity"`），默认 0.1 |
| `sheet` | `{ grid: [列, 行], frames?, fps?（不写按寿命播完）, randomStart?, blend? }` |
| `soft` | 软化距离 |
| `lit` | 受光 |
| `collision` | `{ ground?, scene?, bounce?, friction?, kill?, radius?, group?, only?: [分组…] }` |
| `subEmitters` | `[{ on: "birth" \| "death" \| "collision", count（最多 10000）, inherit?, emitter: { …同 Emitter3dNode 的参数 } }]`，逐项解析；按下标对应 |

- 原来的字段照旧严格（写错整个节点跳过，和以前一样）。
- 只改参数时保留已经发出的粒子（包括子发射器的；子发射器个数变了时多的池子丢掉、少的补上）；换了贴图照旧重来。

```tsx
emitters3d: [{
  id: "firework", y: 40, rate: 0.6, lifetime: [1.2, 1.2], speed: [220, 260], spread: 8, gravity: [0, -200, 0],
  facing: "velocity", stretch: 0.05, blend: "add",
  subEmitters: [{ on: "death", count: 80, emitter: {
    shape: { kind: "sphere", radius: 4, surface: true }, emitDirection: "normal", spread: 0, speed: [80, 120],
    lifetime: [0.8, 1.4], drag: 1.5, gravity: [0, -60, 0], blend: "add",
    colorGradient: [[0, "#ffffffff"], [0.3, "#ffd36bff"], [1, "#ff3d2000"]],
  } }],
}]
```

### 示例（平台跳跃）

- 第一关篝火（`campfireAt` 返回几个发射器）：
  - 火苗是 4 × 4 的序列帧（`tools/make_flame.py` 生成 `flame.png`），按寿命播完、随机起始帧、相邻帧混合；
  - 火星沿速度拉长，碰到地面死掉，落地处的子发射器溅出一小撮火花。
- 3D 试验场（按 G）：
  - 烟花：一发往上飞，死的时候球面炸开（子发射器、`emitDirection: "normal"`），颜色渐变、阻尼；
  - 冲击波：水平平铺、从小到大、淡出，贴着地面一圈一圈往外扩；
  - 软粒子的烟：受光、湍流、大小曲线，贴着地面和石头不一刀切。

## 不做

- GPU 上模拟（计算着色器）、GPU 排序：粒子还在 CPU 上，几千个以内够用。
- 粒子投影子、点光源和聚光灯照粒子、粒子之间互相碰撞。
- 网格粒子（每个粒子是一个模型）。
- 软粒子在 GL 后端上（退成硬边，见上）；和精灵混排时精灵的深度不参与软化。
- 软粒子按需换回不能读的深度纹理：出现过一次软粒子以后一直带 `TEXTURE_BINDING`。
- 子发射器按 id 对应：还是按下标，调换顺序会换掉已经发出去的子粒子的参数（见上）。
- 带子用单独的顶点格式（见「性能」）。
- 场景查询排除某个实体自己的碰撞体：只有按分组过滤（`group`、`only`）。
- 碰撞事件按法向速度的阈值过滤：只看上一帧是不是已经贴着表面（阈值和世界单位的大小有关，不好给默认值）。
- 加载失败的 glTF 重试：资源服务器同一个 uri 只加载一次。
- 碰撞的连续检测：一帧走太远的粒子只按起止两点的射线查，薄的东西可能穿过去；被查询上限挡掉的粒子这一帧只查地面。
- 粒子数的硬上限：`limit` 是软的，这一帧开头算剩多少，同一帧里死掉的粒子腾出的位置下一帧才能用。
- 拖尾（`Trail3d`）的软化、受光、序列帧：带子还是原来的样子。

## 变异表

改掉下面这些，都有测试报错（改完还原，再 touch 文件）：

| 改了什么 | 哪个测试报错 |
| --- | --- |
| 序列帧不混相邻帧（`mix` 恒为 0） | `sheet` 的两个选帧测试 |
| 按寿命播时不停在最后一帧（绕回第一帧） | `lifetime_mode_plays_through_once` |
| 曲线不插值、只取前一个键 | 曲线、渐变的测试，`billboards_follow_curves_facing_sheets_and_flags` |
| 球体里取点不开立方根（往中心挤） | `spheres_on_the_shell_or_filling_the_volume` |
| 网格三角形不按面积、按个数取 | `meshes_pick_triangles_by_area` |
| `emitDirection: normal` 用 direction 不用法线 | `directions_follow_the_mode` |
| 不乘阻尼 | `drag_and_speed_curves_slow_particles_down` |
| 不加湍流 | `noise_pushes_particles_around` |
| 场景查询不看每帧上限 | `scene_queries_are_capped_and_take_turns` |
| `kill` 不起作用 | `falling_onto_the_ground_kills_and_splashes` |
| 反弹不翻法向分量 | `bounce_flips_the_normal_part_and_rubs_the_rest`、`bouncing_flips_and_keeps_particles_above_the_ground` |
| 子发射器多套一层 | `the_total_budget_and_the_nesting_depth_are_capped` |
| 不看总数软上限 | 同上，`the_soft_limit_counts_every_emitter_and_feeds_the_stats` |
| 碰撞的子发射器不转到碰到的面的法线 | `splashes_point_along_the_normal_of_what_was_hit`（补的，原来只测了地面，法线正好是 +Y） |
| 子粒子不继承父粒子速度 | `children_inherit_the_parent_velocity` |
| 统计里存活数恒为 0 | `the_soft_limit_counts_every_emitter_and_feeds_the_stats` |
| 拉长不乘速度 | `velocity_facing_stretches_along_the_velocity` |
| 水平朝向退成朝相机 | `horizontal_and_fixed_facings_ignore_the_camera` |
| 顶点缓冲每帧新建 | `buffers_are_reused_from_frame_to_frame`（补了一帧少的：原来每帧一样多，新建的容量也一样） |
| GL 上不去掉读深度的那段着色器 | `the_gl_shader_does_not_read_the_scene_depth` |
| 着色器 `soft_fade` 恒为 1 | 离屏 `soft_particles_fade_near_opaque_surfaces` |
| 着色器算「离后面多远」时符号反了 | 离屏两个软粒子测试 |
| 着色器方向光不包裹（`n·l` 不乘 0.5 加 0.5） | 离屏 `lit_particles_follow_the_directional_light` |
| 着色器不看 `lit`，都受光 | 同上（不受光的应是白的） |
| 着色器不混下一帧 | 离屏 `sprite_sheet_frames_pick_their_cell_and_blend` |
| 有软粒子时不拆出只读深度的第二个 pass | 离屏两个软粒子测试 |
| 场景不读 `soft` | `new_emitter_fields_parse_into_the_emitter` |
| 固定朝向的欧拉角不换弧度 | 同上 |
| 子发射器的 `emitter` 不递归转换 | 同上 |
| 性能面板不记粒子数 | `nge-ecs` 的 `perf` 两个测试 |
| 子发射器的内层循环不按剩下的预算截、跑满 `count` 次 | `huge_sub_counts_stop_at_the_budget`（墙钟期限） |
| 场景里不截 `count` | `new_emitter_fields_parse_into_the_emitter` |
| 场景查询不按粒子数分（谁在前面谁全拿） | `scene_queries_are_shared_by_every_pool` |
| 分查询时不给每个池子保底一次 | 同上 |
| 算要查的粒子数时不算子池子 | 同上 |
| 贴着表面时每帧照发碰撞 | `resting_particles_splash_once` |
| 离开表面时不清 `touching`（弹起来再落下不算新的碰撞） | 同上 |
| 这一帧没轮到查场景也清 `touching` | `particles_pressed_on_a_wall_splash_once_even_when_queries_take_turns` |
| 网格没加载时照样往下走（吃掉 burst） | `bursts_wait_for_the_mesh`、`mesh_bursts_wait_for_the_model_and_the_cache_follows_the_emitters` |
| 加载完但没有三角形的网格也一直等 | `mesh_bursts_wait_for_the_model_and_the_cache_follows_the_emitters` |
| 模型编号还不存在时缓存成失败 | 同上 |
| 不清没人用的缓存 | 同上 |
| 网格法线先除缩放再乘逆转置 | `mesh_normals_survive_rotation_and_uneven_scale` |
| 湍流的解析偏导写错一项 | `the_curl_matches_finite_differences_of_the_potentials`、`curl_noise_is_smooth_bounded_and_divergence_free` |
| 格点不按 256 取模 | `the_field_repeats_and_scrolling_stays_precise` |
| 滚动偏移用 f32 算 | 同上 |
| 场景查询不按分组过滤 | `scene_collisions_skip_their_group_and_colliders_they_start_in` |
| 出生在碰撞体里时照样当碰到 | 同上 |
| `dropped` 也算单个发射器到 `max` 挡掉的 | `huge_sub_counts_stop_at_the_budget` |
| 排序平局不按放进来的顺序 | `ties_keep_the_order_they_were_pushed_in` |
| 随机起始抽到第 0 帧时当成不循环 | `lifetime_mode_plays_through_once` |
| 固定朝向的欧拉角按 XYZ | `new_emitter_fields_parse_into_the_emitter` |
| `only` 的分组不换成位 | 同上 |
| 深度纹理一直带 `TEXTURE_BINDING` | `the_depth_becomes_readable_only_once_soft_particles_show_up` |
| 软粒子没了就换回不能读的（来回重建） | 同上 |

等价的两处（改了行为不变）：
- 预算用完后不跳出事件循环：内层循环次数是 `min(count, 还能发的)`，已经是 0，只是多走几遍事件。
- `事件数 × count` 不用饱和乘法：64 位上 `usize` 装得下（事件数不会超过 2³²）。

## 验收

- 单元测试：形状取样分布、曲线求值、选帧、拉长朝向的几何、反弹、子发射器递归上限和总数上限、缓冲复用（容量不再增长）、场景解析。
- 离屏测试：软粒子靠近表面变淡、软粒子照样被模型挡住且线照画、正交相机和精灵混排时照样软化、拉长的方片沿速度、序列帧选到对的格子和混合、受光的随方向光变亮。
- 深度纹理：没有软粒子时不带 `TEXTURE_BINDING`，第一次有软粒子时重建、之后不来回重建（有 GPU 时）。
- 平台跳跃：`App.test.tsx` 里篝火的序列帧火苗和会溅起火花的火星；`lab.test.tsx` 里烟花、冲击波、烟的参数。
- 还没手测：篝火的序列帧火苗、火星溅起、试验场的烟花、冲击波、烟在 Mac 上的样子；iPhone、Android 上的性能。

## 修过的问题

- 子发射器的 `count` 很大时，预算只剩几个也要把内层循环跑满 `count` 次（几亿时一帧要一秒）：循环次数按还能发的截，预算用完跳出；个数相乘用饱和乘法；场景里 `count` 最多 10000。
- 每帧的场景查询数被排在前面的发射器全部用掉，后面的发射器（和子发射器）永远查不到：按各池子要查的粒子数分，每个池子保底一次。
- 停在表面上的粒子每帧都算一次碰撞，碰撞子发射器一直喷：只有刚碰上那一帧算，贴着的只修正位置和速度。
- 网格还没加载时一次性发射器的 `burst` 和攒着的 `rate` 被吃掉，`despawn_when_done` 的发射器马上被删：没加载好时什么都不消耗；加载完没有三角形的才放弃。
- 模型编号还不存在（之后才加进来的模型）时表面被缓存成失败，再也不发；发射器没了缓存也不清：不存在的不缓存，没人用的清掉，没有网格发射器时系统直接返回，不再每帧新建数组。
- 固定朝向的欧拉角按 XYZ 转，和模型的 `rotation`（YXZ）不一样：改成一样。
- 深度纹理一直带 `TEXTURE_BINDING`，没有软粒子的场景在移动端也多付一次深度存储：第一次有软粒子时才重建成能读的。
- 部件有旋转、缩放又不均匀时，网格发射形状的法线不垂直于表面：改成先乘部件的逆转置再除以缩放。
- 湍流每个粒子每帧算 18 次值噪声（144 次哈希），滚动用 f32 的秒数，跑久了没有小数：解析求导，一个格点哈希一次出三个值（8 次哈希），格点每 256 格重复，滚动偏移用 f64 取模。
- 场景碰撞不能过滤，出生在碰撞体里的粒子被它挡住停在里面：加 `group`、`only`，碰到的距离是 0 时不算。
- 开了 `collision.scene` 却没有 3D 物理时什么都不说：warn 一次。
- `ParticleStats::dropped` 把单个发射器到 `max` 挡掉的也算进去，和注释不符：只算总数上限挡掉的。
- 一样远的透明方片排序后先后不定：比较时最后按放进来的下标，改用 `sort_unstable_by`。
- 按寿命播、随机起始的序列帧抽到第 0 帧时被当成不循环，停在最后一帧：按 `random_start` 判断。
- 文档里的 `curl(p, t)`、`frame(…)` 和代码对不上，湍流的默认值没写：改成 `curl(p)`、`SpriteSheet::pick`，写上默认 1、1、0。
