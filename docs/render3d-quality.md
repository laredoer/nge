# 3D 画质：天空盒、环境光照、雾、软阴影、多灯投影、级联过渡

> 前置：3D 渲染补齐（[render3d-more.md](render3d-more.md)）、压缩纹理（[compressed-textures.md](compressed-textures.md)）、场景和资源流程（[scene-flow.md](scene-flow.md)）。

## 目标

3D 场景现在看着很「平」：
- 背景只有清屏色，没有天空；
- 环境光是一个常量（`albedo × ambient`），金属、光滑的东西没有反射，背光面一片死灰；
- 没有雾，远处的东西和近处一样清楚，场景边缘生硬地断掉；
- 影子是 3×3 PCF，边缘一格一格的，没有半影；
- 聚光灯、点光源只有第一盏能投影子；
- 方向光的级联之间是硬切，交界处影子的清晰度突然变化。

**验收标准**：
- 天空：
  - 立方体贴图（6 张 PNG，或一张 KTX2 立方体贴图）和程序天空（上、地平线、下三色渐变加太阳）二选一；
  - 画在所有不透明物体后面，不写深度。
- 环境光照：
  - 从天空算出球谐（SH9）做环境漫反射；
  - 预滤波的 mip 链做环境镜面反射，粗糙度越大取越高的 mip，配一张 BRDF LUT（split-sum）；
  - 叠加在原来的常量环境光上，漫反射、镜面强度分开调。
- 雾：
  - 距离雾，线性、指数、指数平方三种；
  - 高度雾（越低越浓，按视线积分）；
  - 颜色可以取天空色；
  - 不透明、半透明网格和粒子方片都受雾影响。
- 软阴影：
  - 3×3 PCF（原来的）、泊松盘 16 点旋转采样、PCSS（方向光，半影随遮挡距离变宽）；
  - 可调深度 bias 和法线偏移。
- 多灯投影：
  - 聚光灯（一层透视图）、点光源（六面写进同一张深度数组）可以有好几盏同时投影子；
  - 投影灯数有上限，按优先级（再按离相机的距离）挑，超出的不投影但照常照明。
- 级联过渡：
  - 方向光相邻级联在交界前一段里混合，最后一段在末尾淡出；
  - 级联按像素格对齐（原来就有），镜头移动时不闪；
  - 可以打开级联调试色。
- 以上都能在 TSX 场景 JSON 里配置，新字段一项写错只丢那一项。

## 做法

### 天空（`Sky`）

```rust
pub struct Sky {
    pub source: SkySource,  // None / Gradient(SkyGradient) / Faces([ImageId; 6]) / Cube(Handle<CubeImage>)
    pub diffuse: f32,       // 环境漫反射强度，默认 1
    pub specular: f32,      // 环境镜面强度，默认 1
    pub background: bool,   // 画天空（默认开）；关掉时只拿来照明
}
```

- 资源，默认 `source = None`：不画天空、没有环境光照，和以前一样。
- 6 张 PNG 用引擎自己的面约定（从里面往外看）：
  - 顺序 +X、-X、+Y、-Y、+Z、-Z，每张是朝那个方向看到的画面，不镜像；
  - 四个侧面的上方是 +Y；
  - +Y 面（抬头）图的上边接 +Z 面的上边（下边接 -Z），-Y 面（低头）图的上边接 -Z 面的下边。
  - CPU 上按 GPU 立方体纹理的采样约定一格一格重排（按方向查用户的图），着色器直接拿世界方向采样。
- KTX2 立方体贴图按 GPU 原生（KTX2 规范、和 Vulkan / D3D 一样）的面方向读，不重排：工具链（`toktx`、`cmft`、Blender 导出等）出的 KTX2 直接能用。所以同样 6 张图，存成 6 张 PNG 和自己拼成 KTX2 看到的可能不一样（侧面左右镜像），要拼 KTX2 请按规范的面方向。
- KTX2 立方体贴图：`nge-assets` 的 `ktx2::parse_cube` 读 `faceCount = 6` 的文件（RGBA8 或块压缩，压缩的在 CPU 上解开；只用第 0 级）。新资源 `CubeImage`。
- 6 个面要一样大、是正方形，不对时 warn 一次（来源换了再 warn），不画天空。
- 程序天空 `SkyGradient { top, horizon, ground, sun, sun_color, sun_size }`：
  - 往上 `mix(地平线, 上, sqrt(y))`，往下 `mix(地平线, 下, sqrt(-y))`；
  - 太阳：视线和太阳方向夹角小于 `sun_size`（半角）的是太阳盘，外面一圈按 `cos^64` 加一点光晕。
  - Rust 和着色器各写一份同样的公式，Rust 那份烘焙环境、测试用。
- 画法：
  - 不透明的网格画完，用一个盖满屏幕的三角形画在远平面（深度比较 `LessEqual`，不写深度），然后再画半透明的网格、粒子、调试线。
  - 像素的视线方向用逆的 `view_proj` 算：正交相机时每个像素方向一样，天空是一个颜色。
  - 天空盖住 z < 0 的背景精灵（它们画在 3D 之前）：要背景精灵就别开 `background`。

### 环境光照（IBL）

- 全在 CPU 上烘焙，只在天空换了（或贴图刚加载完）时做一次，GPU 只采样：WebGPU、WebGL2、老的 Android 都不用计算着色器、storage texture、cube array。
- 烘焙在后台线程（wasm 上没有线程，同步做）；烘焙好以前接着画旧的天空和环境光。资源 `SkyBake { ready, count }` 每帧写：`ready` 是没有在加载或烘焙的，`count` 是开始过几次烘焙。
- 程序天空的太阳不进烘焙：烘焙时去掉太阳，太阳在 `fs_sky` 里每帧按当前方向画。转太阳（日夜变化）不会重新烘焙。
- 先把天空重采样成一个 64 × 64 × 6 的线性颜色立方体：程序天空每格取格子中心的方向；贴图先在 64 × 2^k（不超过原图、最多 256）上每格平均最多 8 × 8 个像素，再逐级 2 × 2 减半到 64。大图每一级都是平均出来的，高 mip 上不会只取到零星几个像素而一闪一闪。
- 漫反射：把它（截到 [0, 1] 以后，和镜面用的 LDR 立方体一致）投影到 SH9（每格按立体角加权），9 个 RGB 系数（辐射度）放进 `Globals`；着色器乘上余弦卷积的系数（π、2π/3、π/4）得到辐照度 `E(n)`，截掉负的（SH9 在亮暗分明的天空上会振铃出负值），漫反射 = `E(n) × albedo / π × (1 - F) × (1 - metallic) × diffuse`。
- 镜面反射：
  - 预滤波的 mip 链 64 → 1 共 7 级，第 `m` 级对应粗糙度 `m / 6`；第 0 级就是原图，其余每格按 GGX 重要性采样 64 个方向（按 pdf 选源图的 mip，少噪点）。
  - BRDF LUT：32 × 32，横轴 `n·v`、纵轴粗糙度，存 split-sum 的 (A, B)，GPU 建好时算一次。
  - 镜面 = `prefiltered(reflect(-v, n), 粗糙度 × 6) × (F0 × A + B) × specular`。
- 原来的常量环境光照旧加上去（要只用 IBL 就把 `ambient` 设成 0）。
- 立方体纹理格式 `Rgba8UnormSrgb`：天空是 LDR 的，超过 1 的部分（太阳）在烘焙时截断；亮度靠 `diffuse` / `specular` 调。

### 雾（`Fog`）

```rust
pub struct Fog {
    pub mode: FogMode,       // None / Linear { start, end } / Exp { density } / Exp2 { density }
    pub color: Vec3,
    pub sky: f32,            // 0–1：颜色往天空色混多少（天空色按视线方向从 SH 算）
    pub height: Option<HeightFog>, // { base, falloff, density }
    pub max: f32,            // 最浓多少，默认 1
}
```

- 距离：透视相机是片元到相机的距离，正交相机是沿相机前方的距离。
- 距离雾：线性 `clamp((d - start) / (end - start))`，指数 `1 - e^(-密度·d)`，指数平方 `1 - e^(-(密度·d)²)`。
- 高度雾：密度 `density × e^(-falloff × (h - base))`，沿视线积分（解析解，视线水平时取极限），透光率和距离雾相乘。
  - 积分写成两项指数相减再除以高度差（提出较大的那项指数，限制在 80 以内），不会出现「下溢的 0 × 上溢的 inf」：相机很高（比如 y = 200）往下看时以前整片是 NaN。结果不小于 0。
- 片元颜色 `mix(颜色, 雾色, 雾量)`；网格、半透明网格、粒子方片的着色器共用同一段 WGSL（`fog.wgsl`）。叠加混合的粒子按 `1 - 雾量` 变淡（加的是光，不往雾色混）。
- 天空不受雾影响。

### 软阴影

`Shadows` 多了几项：

```rust
pub struct Shadows {
    // 原来的 size、cascades、distance、local_size
    pub filter: ShadowFilter,  // Pcf（默认，原来的 3×3）/ Poisson / Pcss
    pub softness: f32,         // 泊松盘半径（阴影图像素），默认 1.5
    pub sun_size: f32,         // PCSS：太阳的角直径（度），默认 1，限制在 [0, 10]
    pub bias: f32,             // 比较前从片元深度里减掉多少（阴影图深度），默认 0
    pub normal_bias: f32,      // 沿法线往外推几个阴影图像素，默认 1.5（原来的常量）
    pub max_lights: u32,       // 聚光灯、点光源最多几盏投影子，0–4，默认 2
    pub blend: f32,            // 级联交界混合的长度（占这一段的比例），0–0.5，默认 0.1
    pub debug_cascades: bool,  // 级联调试色
}
```

- `Poisson`：16 个点的泊松盘，半径 `softness` 像素。每个像素转一个角度，角度取 4 × 4 Bayer 矩阵，按片元落在阴影图的哪一格查（光空间），不按屏幕像素：镜头动时噪点贴着影子不跟着游，也没有交错梯度噪声那种斜条纹。泊松盘的数在 Rust 里，建管线时替换进着色器源码，两边不会不一致。
- `Pcss`（只对方向光；聚光灯、点光源按 `Poisson`）：
  - 先在 16 个点里找挡光的（深度比片元小的），没有就算照到；比较时按接收面的斜率把片元深度挪到每个采样点上（从法线和这一段的光矩阵算，不用导数），斜着的地面不会把自己当成遮挡物；
  - 半影直径 = `(片元深度 − 平均遮挡深度) × 这一段的深度范围 × tan(太阳角直径) / 一个像素的世界尺寸`，泊松盘要的是半径，取一半，限制在 [1, 16] 像素；
  - 再按这个半径做泊松 PCF。离遮挡物越远影子越虚。
  - 开销：每个像素 16 次 `textureLoad`（找遮挡）+ 16 次比较采样（PCF），共 32 次；`Poisson` 16 次，`Pcf` 9 次。
- bias：法线偏移按 `normal_bias × 一个像素在世界里多大`（透视的按到灯的距离放大）；深度 bias 在比较前减掉。光栅化时的斜率偏移照旧。
  - 深度 bias 是阴影图里的深度。方向光是正交投影，深度线性，一个值处处一样；聚光灯、点光源是透视投影，深度不线性，同一个 bias 在近处是很小的距离、在远处是很大的距离。所以不建议调 `bias`（默认 0），有自阴影条纹先调 `normal_bias`。

### 多灯投影

- `Globals` 里每盏聚光灯、点光源各记一个「阴影从第几层开始」（0 是不投影）。聚光灯占 1 层，点光源占 6 层（+X、-X、+Y、-Y、+Z、-Z），合用一张 `local_size × local_size × 层数` 的深度数组。
- 数组的层数只涨不缩（灯来回切换时不反复重建），至少 7 层，而且不是 6 的倍数：GL 后端把「正方形、层数是 6 的倍数」的 2D 数组当成立方体纹理，1 层的当成普通 2D 纹理，都和着色器里的 `texture_depth_2d_array` 对不上。方向光的数组在 GL 上也一样补层。
- 显存上限：`local_size² × 层数 × 4` 字节超过 64 MB 时边长减半（最小 64），warn 一次。默认 512、最多 25 层（24 层补成不是 6 的倍数）约 26 MB。
- 挑灯（`shadow_soft.rs` 的 `pick`，纯函数）：开了 `shadows` 的聚光灯、点光源按优先级从大到小，同优先级按离相机近的、再按查询顺序；挑够 `max_lights` 盏为止。优先级是灯实体上的 `ShadowPriority(i32)` 组件，没有算 0。
  - 滞后：上一帧投影子的灯，距离按 0.9 倍算，新灯要近出 10% 以上才换掉它；镜头在两盏灯中间来回走时影子不来回跳。
- 光矩阵最多 4 × 6 = 24 个（`MAX_LOCAL_LAYERS`）。阴影 pass 每层一遍，照旧只画包围盒和这一层视野相交的。
- 超出上限的灯照常照明，只是没有影子。

### 级联过渡

- 级联按像素格对齐、边长按外接球（原来的，见 [render3d-more.md](render3d-more.md)），镜头平移、转动时影子不闪。
- 混合（`shadow_soft.rs` 的 `cascade_blend` 是测试用的参考实现，着色器 `lit_directional` 里一样的一份）：片元落在第 `c` 段的最后 `blend × 段长` 里时，按位置线性混第 `c + 1` 段的结果；最后一段混的是「照到」，影子在 `distance` 前淡出，不再一刀切。
- 调试色：按段红、绿、蓝、黄乘到最终颜色上，交界处是两种颜色按混合权重混出来的。

### 着色器和绑定

- `Globals` 多了：逆的 `view_proj`、每盏聚光灯 / 点光源的阴影层、每盏聚光灯的法线偏移、软阴影参数、每段的 PCSS 系数、天空参数、SH9、雾。
- 主 pass 的全局 bind group 多 4 个绑定：环境立方体、天空立方体、BRDF LUT、线性采样器。没有天空时绑 1 × 1 的空立方体。
- 粒子方片的 uniform 多了雾和 SH（取天空色用），不用采样立方体。

### 场景和 TSX

```ts
scene.sky = {
  cube?: string | string[6];     // 一张 KTX2，或 6 张图（+X、-X、+Y、-Y、+Z、-Z；asset:/ 路径或注册过的纹理名）；不写就是程序天空
  top?: string; horizon?: string; ground?: string;   // 程序天空的三色
  sun?: [number, number, number];  // 阳光照射的方向（和方向光的 direction 一样，太阳画在反方向）；不写跟第一盏方向光，[0, 0, 0] 没有太阳
  sunColor?: string; sunSize?: number;  // 太阳色、太阳盘半角（度，默认 1.5）
  diffuse?: number; specular?: number;  // 环境光照强度，默认 1
  background?: boolean;                 // 默认 true
};
scene.fog = {
  mode?: "none" | "linear" | "exp" | "exp2";  // 不写时：写了 start / end 是 linear，写了 density 是 exp，都没写只有高度雾
  start?: number; end?: number;       // 默认 0、100
  density?: number;                   // 默认 0.02
  color?: string;
  sky?: number;                       // 雾色往视线方向的天空色混多少（0–1）
  height?: { base?: number; falloff?: number; density?: number };  // 默认 0、0.5、0.05
  max?: number;                       // 默认 1
};
scene.shadows = {
  filter?: "pcf" | "poisson" | "pcss"; softness?: number; sunSize?: number;
  bias?: number; normalBias?: number; maxLights?: number;
  blend?: number; debugCascades?: boolean; size?: number; localSize?: number;
};
// 分几段、覆盖多远照旧写在 camera3d.shadowCascades / shadowDistance
lights[i].shadowPriority?: number;
```

- 宽松规则（`lenient.rs` 的 `Fields`）：`sky`、`fog`、`shadows` 各自单独解析（不是对象时整项当没写）；里面**每个字段**也单独解析，写错的那个字段 warn 后当没写，其余照用（`fog.height` 写错时整个高度雾当没写）；灯的 `shadowPriority` 写错只丢这个字段，灯照常。
- 同步（`nge-scene` 的 `quality3d.rs`）：各自的声明变了才写资源。`sky`、`fog` 整个按声明重算（去掉了回到默认：没有天空、没有雾；场景从来没写过的不碰，Rust 设的留着）；`shadows` 里声明了的字段写进去，上次声明、这次去掉的回到默认，没声明过的留着 Rust 设的（和 `camera3d.shadowCascades` 一样）。程序天空没写 `sun` 时第一盏方向光的方向变了也重算。`sky.cube` 的 `asset:/` 路径异步加载，加载完才烘焙，没加载完时画旧的天空；换关过场（`transition`）的预加载也等 `sky.cube` 和 6 张面图。
- 天空写错时整个 `sky` 当没写，不退回程序天空：`cube` 不是字符串 / 6 个字符串的数组，或者面图的纹理名没注册，warn 一次。
- 写了不认识的字段（拼错之类）warn 一次，其余照用。`shadowPriority`、`maxLights`、`size`、`localSize` 要是整数，写小数 warn 后当没写。

### 降级

- 不用计算着色器、storage texture、cube array：烘焙全在 CPU，GPU 只要普通的立方体纹理和 2D 深度数组。
- 会跑到 GL（GLES / WebGL2 走法）上的情况：web 只走 WebGPU（[web.md](web.md)），Android 只走 Vulkan；但桌面上非 Apple 的默认后端集合里有 GL，Linux / Windows 上没有 Vulkan、DX12 可用的机器会退到 GLES。
- GL 上的处理：
  - 着色器换一份：PCSS 找遮挡物要用 `textureLoad` 读深度纹理，naga 翻不成 GLSL（同一张深度纹理又比较又不比较地读在 GLSL ES 里不行），GL 上 `pcss` 换成直接调 `filtered`，`Pcss` 退成 `Poisson`（方向光也按泊松盘）。建管线时看 adapter 的后端选源码。
  - 阴影数组的层数避开 1 和 6 的倍数（见「多灯投影」），不会被当成 2D 纹理或立方体纹理。
- 投影的灯多了阴影 pass 也多（点光源一盏 6 遍）：手机上建议 `max_lights` 用 1–2。
- 大的立方体贴图烘焙要时间：在后台线程做，不卡帧；wasm 上同步做，会卡一下。

## 不做

- HDR 天空（`.hdr` / `.exr`）、色调映射。
- 天空颜色随时间变化的动态烘焙：换一次颜色烘焙一次（后台线程，烘焙好之前是旧的）；只转太阳不重新烘焙，但环境光照里没有太阳（LDR 立方体里太阳本来也被截到 1）。
- GL 上的 PCSS（退成泊松盘）；GL 后端没有专门跑过离屏测试，只测了着色器能翻成 GLSL ES 3.1。
- 阴影图按光源距离的自适应深度 bias（透视阴影的常量 bias 见「软阴影」）。
- 蓝噪声纹理（用 4 × 4 Bayer 代替，不用多绑一张纹理）；时间上的抖动累积（没有 TAA）。
- 局部反射探针、屏幕空间反射、环境光遮蔽（SSAO）。
- 体积雾、雾里的光柱；天空本身被雾盖住。
- 聚光灯、点光源的 PCSS（按泊松 PCF）；方差阴影（VSM / ESM）。
- 阴影图集的动态分区（每盏局部光一样大）；超过 4 盏局部光同时投影。
- 立方体贴图的 mip 从文件读（KTX2 只用第 0 级，mip 自己算）。

## 验收

- `nge-render3d` 单元测试：
  - SH9：常量天空的辐照度处处是 π × 颜色；上白下黑的天空朝上的辐照度接近 π、朝下接近 0；SH 求值的天空色和原来的颜色大致一样。
  - 立方体面：方向 → (面, u, v) 和 (面, u, v) → 方向互逆；用户约定的面重排后，朝 -Z 看到的是 -Z 面、左右不镜像。
  - 预滤波：第 0 级等于原图；粗糙度越大越接近平均色。
  - BRDF LUT：A + B ≤ 1；和 Karis 的近似公式差得不多；粗糙度 0、正对时 A ≈ 1。
  - 程序天空的颜色公式：正上方是 `top`、地平线是 `horizon`、太阳方向是太阳色。
  - 雾：线性的 start 前 0、end 后满、中间一半；指数单调；高度雾越高越淡、视线水平时连续；`max` 截住；相机在 y = 200 看地面不是 NaN、和解析值一致，两头都在很深的地方是满雾。
  - 挑灯滞后：上一帧投影的灯，新灯近不到 10% 换不掉它。阴影数组层数：只涨、至少 7、不是 6 的倍数。显存上限：超过时边长减半。
  - 天空烘焙：转太阳烘焙出来的东西不变；烘焙不带太阳、截到 [0, 1]；大图逐级平均，高 mip 和小图的结果接近。
  - GL：着色器能翻成 GLSL ES 3.1；GL 那份没有 `textureLoad` 找遮挡物。
  - 泊松盘：都在单位圆里、两两不太近；旋转后半径不变。
  - 级联混合：段中间不混，交界前 `blend` 里从 0 涨到 1，最后一段混向照到。
  - 挑灯：优先级高的先、同优先级近的先、上限以外的不投；点光源 6 层、聚光灯 1 层，层号连续。
  - PCSS 系数：深度范围按矩阵算对。
- 离屏渲染测试：
  - 程序天空：没东西的地方是天空色不是清屏色，上方偏 `top`，太阳方向是太阳色；物体挡住的地方是物体。
  - 立方体贴图（6 张不同颜色的面，6 张图和 KTX2 两条路）：朝 -Z 看左右不镜像，朝 +X、+Z 看是那一面。
  - 环境漫反射：没有灯、环境光 0 时，0.5 灰的天空把白球照成约 0.48；`diffuse = 0` 时是黑的；上亮下暗的天空照得球上半边亮。
  - 环境镜面：光滑金属球 `specular = 0` 是黑的，`= 1` 映出天空；0.5 灰的天空下白色金属球中间的亮度对得上 split-sum（粗糙度 0.5 约 179、1 约 110，sRGB）；上白下黑的天空下粗糙度 0.1 的球上下反差比粗糙度 1 的大。
  - 天空：转太阳 20 帧只烘焙一次、太阳确实在动；6 张 PNG 按引擎约定、KTX2 按规范（按 8 个象限各涂一个颜色，往 8 个方向看）；新天空的贴图加载、烘焙好以前画旧的。
  - 高度雾：相机在 y = 200 往下看，地面按正确的雾量发红（以前是 NaN）；相机和地面都在很深的地方是满雾。
  - 雾：远处的地面趋向雾色，近处不变，只开高度雾也盖住低处；半透明网格、普通和叠加的粒子方片也被雾盖住。
  - 软阴影（一堵薄墙斜照出的影子，量影子侧边的过渡像素）：`Poisson` 比 `Pcf` 宽；PCSS 离墙边远的地方比墙根宽，泊松盘两处一样宽。
  - 多灯：两盏聚光灯都投影子；`max_lights = 1` 时第二盏不投影但照亮；优先级高的那盏投影子；点光源（6 层）和聚光灯一起投影子；聚光灯优先、点光源从第 1 层开始时也读对自己的层。
  - PCSS 的半影宽和太阳角直径对得上（半径不是直径）。
  - 级联调试色：近处偏红、远处偏绿，混合带里混色；`blend = 0` 时同一处还是红。
  - 最后一段淡出：两段、`distance` 25 时，中间方块的影子在末尾的混合带里大半淡掉，近处照常。
- 场景层（`quality3d.rs`）：写错一个字段只丢它、整项不是对象时当没写；`cube` 写错或面图没注册时整个 sky 当没写；多余的字段 warn 一次；场景从没写过 `sky` / `fog` 时 Rust 设的留着；过场等天空立方体和面图；雾的模式按写了什么推断；声明没变不覆盖 Rust 改的、去掉的回到默认；太阳跟着方向光；`sky.cube` 6 张图和 KTX2；`shadowPriority` 挂到灯上、去掉就没有。
- 试验场：`lab.test.tsx` 查场景里的天空、雾、阴影和三盏投影的灯；`src/lab.rs` 查 C 键只在试验场里开关级联调试色、离开时关掉。
- 平台跳跃试验场（按 G）：程序天空和环境光、雾、PCSS 软阴影、一盏投影的点光源和一盏投影的聚光灯、三段级联带过渡；按 C 切换级联调试色。
- 还没手测：画面只在离屏测试里读过像素，没有在窗口里看过。

### 变异验证

| 改坏的地方 | 挂的测试 |
| --- | --- |
| 级联混合带宽度置 0（`shader.wgsl` 的 `cascade_of`） | 离屏 `cascade_debug_colors_blend_across_the_seam` |
| 挑灯的优先级反过来排（`shadow_soft.rs` 的 `pick`） | 单元 `picks_by_priority_then_distance_then_order`、`point_lights_take_six_layers`；离屏 `several_spot_lights_cast_shadows_up_to_the_limit_by_priority` |
| 投影灯数不看 `max_lights`（`shadow.rs`） | 离屏 `several_spot_lights_cast_shadows_up_to_the_limit_by_priority` |
| 着色器的线性雾最多 0.3（`fog.wgsl`） | 离屏 `fog_tints_far_things_toward_its_color` |
| Rust 的线性雾截到 0.5（`fog.rs`） | 单元 `linear_fog_starts_and_ends` |
| 叠加粒子不乘 `1 - 雾量`（`billboards.wgsl`） | 离屏 `fog_covers_blended_meshes_and_billboards_too` |
| 泊松盘半径不乘 `softness`（`shader.wgsl` 的 `filtered`） | 离屏 `soft_filters_widen_the_shadow_edge` |
| PCSS 找遮挡物的比较反过来（`shader.wgsl` 的 `pcss`） | 离屏 `pcss_penumbra_grows_with_the_distance_to_the_blocker` |
| SH 余弦卷积系数换成 1（`fog.wgsl` 的 `sh_irradiance`） | 离屏 `the_sky_lights_objects_by_its_irradiance` |
| 天空的深度比较改成 `Always`（`sky_gpu.rs`） | 离屏 `gradient_sky_fills_the_background_behind_objects` |
| 用户 -Z 面左右镜像（`ibl.rs` 的 `USER`） | 单元 `user_faces_are_what_you_see_looking_out`；离屏 `png_faces_show_what_you_see_looking_out_without_mirroring` |
| `shadows` 里去掉的字段不回默认（`quality3d.rs`） | 场景 `scene_writes_sky_fog_and_shadows_only_when_declared_changes` |
| 不给灯挂 `ShadowPriority`（`sync_nodes.rs`） | 场景 `shadow_priority_goes_on_the_light` |
| 逐字段宽松改成写错就失败（`lenient.rs` 的 `Fields::get`） | 场景 `a_wrong_field_drops_only_that_field` 等 4 个 |
| 点光源的层不加起始层：`first + face` → `face`（`shader.wgsl` 的 `lit_point`） | 离屏 `a_point_light_after_a_spot_light_reads_its_own_layers` |
| BRDF LUT 的两个坐标对调（`shader.wgsl` 的 `environment`） | 离屏 `metal_reflects_the_sky_by_the_split_sum`、`rough_metal_blurs_the_reflection` |
| 镜面 LOD 不乘粗糙度（总取最糊 / 总取第 0 级，各跑一次） | 离屏 `rough_metal_blurs_the_reflection` |
| 最后一段淡出到全黑：`next = 0.0`（`lit_directional`） | 离屏 `the_last_cascade_fades_out_to_lit` |
| PCSS 半影用直径不用半径 | 离屏 `pcss_penumbra_grows_with_the_distance_to_the_blocker` |
| 高度雾两项分别截到 ±80 再相减（Rust / 着色器各一次） | 单元 `height_fog_from_a_high_camera_stays_finite`；离屏 `height_fog_from_a_high_camera_is_not_nan` |
| 挑灯没有滞后（`shadow_soft.rs` 的 `pick`） | 单元 `the_lights_shadowed_last_frame_keep_their_shadow_until_clearly_beaten` |
| 阴影数组层数可以是 6 的倍数 / 会缩（`array_layers`，各一次） | 单元 `shadow_arrays_only_grow_and_avoid_cube_shapes` |
| 局部阴影不看显存上限（`fit_local_size`） | 单元 `the_local_shadow_maps_stay_within_the_budget` |
| 太阳算进烘焙键（`sky_gpu.rs` 的 `bake_key`） | 单元 `turning_the_sun_does_not_change_what_is_baked`；离屏 `turning_the_sun_does_not_rebake_the_sky` |
| 烘焙带上太阳 / SH 用没截断的值（`bake`，各一次） | 单元 `baking_leaves_the_sun_out_and_clamps_like_the_cube` |
| 大天空图不逐级平均（`env_base`） | 单元 `big_skies_are_averaged_down_not_point_sampled` |
| KTX2 按引擎约定读 | 离屏 `ktx2_cubes_follow_the_spec_layout` |
| 6 张 PNG 按 GPU 约定读 | 离屏 `png_faces_show_what_you_see_looking_out_without_mirroring` |
| 新天空加载时扔掉旧的（`prepare` 的 `Pending`） | 离屏 `the_old_sky_stays_until_the_new_one_is_ready` |
| GL 上不换掉 PCSS（`renderer.rs` 的 `shader_source`） | 单元 `the_gl_shader_drops_the_pcss_blocker_search` |
| 多余字段不报（`Fields::finish`） | 场景 `unknown_fields_are_warned_once` |
| `cube` 写错退回程序天空（`quality3d.rs`） | 场景 `a_bad_cube_drops_the_whole_sky` |
| 场景没写 `sky` / `fog` 也写资源（各一次） | 场景 `rust_sky_and_fog_survive_while_the_scene_never_declares_them` |
| 过场不等天空立方体（`transition.rs`） | 场景 `the_switch_waits_for_the_sky_cube_and_faces` |

没有测试挂的（都实际改坏跑过一遍离屏和单元测试）：
- 遮挡搜索去掉接收面斜率补偿、泊松盘不旋转（Bayer 换成 0）：离屏只读得到整块的亮暗，读不出斜面上的细条纹和噪点形状。
- 着色器里辐照度去掉 `max(·, 0)`：烘焙前已经截到 [0, 1]，测试用的天空上 SH9 没有振铃出负值。
- 高度雾提出的指数不限制在 80、最后不截 0：新写法下两项都不超过 1、结果本来不小于 0，去掉了只是在极端值上得到 inf（雾量照样是 1），是兜底。

## 修过的问题

- 相机很高时高度雾整片是 NaN：原来的解析式里有一项下溢成 0、另一项上溢成 inf，相乘是 NaN。改成两项指数相减、提出较大的那项（指数最大 80），Rust 和 `fog.wgsl` 一起改；相机在 y = 200 往下看、两头都在 base 下面很深时都有单元和离屏测试。
- 转动程序天空的太阳每帧都重新烘焙（日夜变化时一直卡）：烘焙不带太阳，太阳在 `fs_sky` 里画。
- GL 后端上多灯阴影纹理被当成立方体：局部阴影数组正好 6 层（一盏点光源）时，GL 按「正方形、6 的倍数层」建成立方体纹理，1 层的建成 2D 纹理。层数现在至少 7、避开 6 的倍数、只涨不缩；方向光的数组在 GL 上也补层。
- 局部阴影图没有显存上限：`local_size = 4096` 加 25 层要 1.6 GB。超过 64 MB 时边长减半并 warn。
- 两盏灯离相机差不多远时，镜头稍微一动投影子的灯就来回换：挑灯加了 10% 的滞后。
- `sky.cube` 的面图有一张没注册时，缺的那面被当成黑的照常烘焙：现在整个 sky 当没写并 warn；`cube` 写错（不是字符串或 6 个字符串）时也一样，不再悄悄退回程序天空。天空用不了的 warn 在来源换了以后会重新报。
- KTX2 立方体贴图按引擎给 6 张 PNG 定的面约定重排，工具链导出的 KTX2 侧面左右反了：KTX2 改按 GPU / KTX2 规范的面方向读，6 张 PNG 照旧按引擎约定。
- 场景从没写过 `sky` / `fog` 时，第一帧同步把 Rust 设的天空、雾清掉了。
- 换天空时新贴图没加载完、烘焙没做完的几帧没有天空（闪成清屏色）：现在一直画旧的；换关过场的预加载也等天空贴图。
- 大立方体贴图烘焙卡帧：挪到后台线程（wasm 照旧同步）；从贴图天空换回程序天空时释放大的天空立方体。
- SH 用的是没截断的天空（太阳的亮度进了漫反射，和截到 1 的镜面不一致），而且 SH9 在亮暗分明的天空上会振铃出负的辐照度：SH 改用截断后的值，着色器里辐照度不小于 0。
- PCSS 把半影直径当半径用，影子虚了一倍；找遮挡物时斜着的接收面把自己当遮挡物：半影取一半，比较时按接收面斜率补偿。
- 泊松盘按屏幕像素的交错梯度噪声旋转，有斜条纹、镜头动时噪点游动：改成按阴影图格子查 4 × 4 Bayer。
- 大的贴图天空在高 mip 上只取到零星几个像素，一闪一闪：先在更大的分辨率上平均再逐级减半。
- 场景的 `sky` / `fog` / `shadows` 里拼错的字段悄悄被忽略：现在 warn 一次。
