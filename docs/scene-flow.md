# 场景和资源流程：预制体、异步预加载、换关过场、PNG 自动 mipmap

> 前置：声明式场景（[m3-tsx.md](m3-tsx.md)）、资源层（[m5-extensions.md](m5-extensions.md)）、关卡切换（[levels.md](levels.md)）、压缩纹理（[compressed-textures.md](compressed-textures.md)）、输入映射（[input-actions.md](input-actions.md)）。

## 目标

做到第二轮为止，场景和资源的流程还缺几块：

- 重复的节点组（一棵树是树干加树冠、一个敌人是身体加眼睛）只能在 TSX 里写函数拼，场景 JSON 和关卡文件里没法复用。
- 资源只能「用到时再加载」：换关时新关卡的贴图、模型一帧一帧冒出来，也没法做加载进度条。
- 换关是硬切：`level` 一变当帧重建，上一关直接消失，新关卡的资源还在路上；levels.md 的「不做」里把过场留给了 TSX 自己拼。
- PNG 贴图只有第 0 级：缩小（镜头拉远、3D 模型在远处）时闪烁、有锯齿。只有 KTX2 自带 mipmap。

**验收标准**：

- 预制体：一个文件（TSX 模块，或 `asset:/` 的 JSON 文件）定义一组节点；场景里按名字或路径实例化，可以平移、按子节点 id 覆盖属性、嵌套；循环引用报错不卡死；TSX 和场景 JSON 都能用。
- 预加载：一组资源（贴图、声音、模型、字体、场景 / 预制体文件）在后台加载，能查「已完成 / 总数」，能等全部完成；失败的算完成并报出来，不卡住进度；TSX 有 `preloader()`。
- 换关过场：`scene.transition` 写了以后，换关时先淡出（淡黑或横向擦除），同时预加载新关卡要用的资源；资源好了、又过了最短时长才真正换关，再淡入；整个过场里游戏收不到输入。
- PNG 自动 mipmap：PNG / JPEG 贴图上传后在 GPU 上逐级生成 mip 链，按线性空间（sRGB 先解码）、按透明度加权求平均；`scene.textures` 能按贴图关掉；KTX2 自带的 mip 不动。
- 平台跳跃：换关有过场和预加载（两种过场轮着用）；背景的树是预制体（TSX 里的「树丛」嵌套 JSON 文件里的「树」）；主角图集关掉 mipmap。

## 做法

### 预制体（`nge-scene` 的 `prefab.rs`）

预制体就是一份「场景片段」：

```json
{
  "sprites": [{ "id": "trunk", "y": 40, "w": 10, "h": 80 }, { "id": "crown", "y": 90, "w": 44, "rotation": 45 }],
  "instances": [{ "id": "nest", "prefab": "nest", "y": 100 }]
}
```

- **能放的列表**：`sprites`、`tilemaps`、`texts`、`emitters`、`bars`、`bubbles`、`joints`，加上嵌套的 `instances`。3D 节点不支持（见「不做」）。
- **定义在哪**：
  - 场景的 `prefabs: { 名字: 预制体 }`。TSX 从一个模块 import 进来放进去（平台跳跃的 `prefabs.ts`）。
  - `asset:/` 路径的 JSON 文件，一个文件一个预制体。用资源层加载（`SceneFile` 资源），加载好以前这个实例先不出现，好了再展开。
- **实例**：场景的 `instances: [{ id, prefab, x, y, z, set }]`。
  - `prefab` 写名字（找 `scene.prefabs`）或 `asset:/` 路径。
  - 子节点的 id 变成 `实例 id/子节点 id`，嵌套时一层层往前加（`grove-0/a/crown`）。
  - `x`、`y`、`z` 加到子节点的位置上：精灵、瓦片地图、粒子发射器、没有 `follow` 和 `screen` 的文字 / 进度条 / 气泡；关节的锚点也加。只平移，不转、不缩放。
  - 引用改名：关节的 `a` / `b`、文字 / 进度条 / 气泡的 `follow`、精灵 `chase` 的 `target`、精灵行为树（`behavior.tree`，一层层往下找）里 `see` / `near` / `chase` 的 `target` 指向预制体里面的节点时，跟着加前缀；指向外面的（比如 `player`）不动。
  - 行为树里 `goto` 的 `x`、`y` 跟着平移（和精灵一样）。
  - `set: { 子节点 id: { 字段: 值 } }` 按字段覆盖（浅合并，对象整个换掉）。嵌套里的写 `a/crown`。覆盖在平移之前做，坐标是预制体里的坐标；外层的覆盖盖过里层。没有这个子节点时 warn。
- **展开时机**：`PreUpdate` 里资源收回之后、换关和同步之前跑 `expand_prefabs`。新场景（`expanded` 标记是假）有实例时，记下各列表原来的长度，展开后接在后面，标记成已展开；后来有预制体文件加载好（`AssetEvent<SceneFile>`），截回原来的长度重新展开一遍。展开后的节点和手写的一样走宽松解析和增量同步。 只有这个场景用到的预制体文件（展开时找过的）加载好或失败了才重新展开，别的 JSON 文件（比如 `preload` 命令载的）不算。
- **出错**：名字找不到、文件加载失败、不是对象、子节点没有字符串 id 都 warn（只说一次）、跳过那一个实例或节点，别的照常。 展开后实例 id 重复、或者同一个列表里展开出来的 id 和别的（包括手写的）撞了，也 warn（同一个列表里后面的盖掉前面的）。
- **循环**：展开时记着当前这条链，碰到链上已有的预制体就 warn「预制体循环：a → b → a」，跳过这一个实例；再加两道保险：嵌套最多 16 层、一个场景最多展开 10000 个实例和节点（合起来算，每展开一个实例也占一个；只有实例、没有节点的一层层扇出也停得住）。

### 预加载

**`nge-assets` 的 `LoadGroup`**（和资源类型无关）：

- `add::<A>(uri, handle)` 记一个句柄（同一个句柄只记一次），`fail(uri, 原因)` 记一个一开始就失败的（比如认不出类型）。
- `progress(world)` 返回 `LoadProgress { done, total, failed: [(uri, 原因)] }`：加载好的和失败的都算 `done`，`finished()` 是 `done == total`。
- Rust 里等全部完成：每帧问 `progress`，或者在加载界面里 `AssetServer::apply(world, true)`。

**`nge-scene` 的 `preload.rs`**：

- 按扩展名认类型：`png` / `jpg` / `jpeg` / `ktx2` 是贴图，`wav` / `ogg` / `mp3` / `flac` 是声音，`gltf` / `glb` 是模型，`ttf` / `otf` / `ttc` 是字体，`json` 是场景 / 预制体文件。认不出的直接记失败。
- JSON 文件加载好以后按场景宽松解析，把它引用的资源（贴图、法线图、字体、模型、背景音乐和 `audio.preload`、预制体文件、`transition.preload`、`textures` 的键）也加进同一组：总数会变大。
- 命令 `preload { id, uris }`：开一组；之后完成数、总数或失败数变了就发事件 `preload { id, done, total, failed: [{ uri, error }], finished }`，第一次检查一定发一次，`finished` 以后这一组扔掉。同一个 `id` 再发一次是重新开始。 同一组里同一个 uri 只算一次（认不出类型、没注册类型的也是）。
- **进度可能倒退**：JSON 文件载好以后它引用的资源才加进来，总数变大，`done / total` 会变小。TSX 的 `preloader()` 给 `fraction()`：只涨不退的比例，进度条用它。
- TSX 的 `preloader()`：引擎 30 秒（`waitMs`）没回进度，这一组按「没回」结束（已经报失败的留着，别的 uri 都记成失败）；视图重建时调 `reset()` 把没完成的都这样结束，不会一直挂着。

### 换关过场（`transition.rs`）

`scene.transition: { kind, color, out, in, min, preload }`：

| 字段 | 默认 | 说明 |
|---|---|---|
| `kind` | `"fade"` | `"fade"` 整屏变成 `color`；`"wipe"` 从左往右盖上，再从左往右揭开 |
| `color` | `"#000000"` | |
| `out` / `in` | 0.3 / 0.3 | 淡出、淡入各几秒（按真实时间，不受 `timeScale` 影响） |
| `min` | 0 | 从开始淡出到真正换关最少几秒 |
| `preload` | `[]` | 另外要等的资源（新场景里看不出来的，比如命令里才播的音效） |
| `timeout` | 10 | 资源最多等几秒（从开始淡出算）：到了还没加载完的记成失败（原因写「超时」），照样换关 |

- **什么时候算换关**：`scene.level` 和已经交给同步的关卡不一样，而且新场景写了 `transition`。第一次声明关卡时（之前没有场景，或者之前的场景都没写 `level`）不淡出，直接从盖满淡入（开场时资源冒出来被盖住）。没写 `transition` 的换关和以前一样当帧重建。
- **按住新场景**：淡出期间（`SceneTransition` 在按住），换关系统和所有同步系统都不跑（条件从 `resource_changed::<Scene>` 换成 `scene_ready`：场景变了且没按住）。过场系统 `run_transition` 自己按 `Scene` 的改动 tick 判断场景变没变。屏幕上还是上一关；`Scene` 资源已经是新的，过场期间 TSX 再发的场景照样接住，最后一份为准。 `scene_ready` 是公开的：自己写的、按 `Scene` 同步的 Rust 系统用它代替 `resource_changed::<Scene>`。每帧都读的东西不能直接读 `Res<Scene>`：2D 光照（`lighting` 每帧摆灯）改成读同步时存下的那份（`SceneLighting`），按住时还是上一关的光照；其余每帧跑的系统都不读 `Scene`。
- **淡出期间**：
  - 盖住的比例 `cover` 按 `1 / out` 每秒涨到 1。
  - 预加载：新场景引用的资源（同上，加上 `transition.preload`）放进一个 `LoadGroup`。过场期间场景又变了，新出现的资源加进去。 盖着的时候又换了一关（A → B → C）：重开一组，不再等 B 的资源（已经过的时间照算）。开始预加载之前先按新场景的 `scene.textures` 设好贴图开关，预加载的贴图一上传就是要的样子。
  - 三个条件都满足才放开：盖满了、这一组全部完成（失败也算；或者过了 `timeout`）、过了 `min`。放开时不再按住、`Scene` 标记成变了，同一帧里换关系统照常重建、发 `levelStart`。
- **淡入**：`cover` 按 `1 / in` 降到 0。淡入期间又换关：从当前的 `cover` 接着淡出。
- **反悔**：淡出期间 `level` 又变回已经在玩的那一关：不重建，放开按住（增量同步），从当前的 `cover` 淡入。
- **画**：`PostUpdate` 里（相机跟随、震屏之后）往 `SpriteInstances` 放一块白色贴图乘上颜色的矩形，大小是视口除以缩放再多 2 point、跟着相机的位置和角度，z 是 `GIZMO_Z` 的两倍（不受光，和 3D 混排时深度贴在最前）。淡黑是整块、透明度乘 `cover`；擦除是只盖住左边 `cover` 那么宽（淡入时盖住右边）。 文字（`scene.texts`、气泡、飘字、性能面板）是另一个渲染器画的，排在精灵后面，所以**画在盖子上面**：过场时屏幕上的字照样看得见（见「不做」）。
- **输入**：过场期间（淡出、等待、淡入）`Input::block(true)`。`nge-ecs` 的 `update_actions`（现在在系统集 `ActionsUpdated` 里）看到被挡住时先 `release_all`：按着的键、指针、摇杆、扳机、手柄按键都当没有（不发松开的边沿），TSX 直接驱动的动作（`act`、虚拟摇杆）也放掉。过场系统排在它前面，开始过场的那一帧就挡住。过场里一直按着的键要松开再按才算。
- **事件** `transition { phase, level, done, total, failed }`：`out`（开始淡出，之后加载进度变了再发）、`in`（换好了开始淡入）、`done`（淡入完）。Rust 读资源 `SceneTransition`（`active()`、`cover()`）。

### PNG 自动 mipmap

- `Image` 加字段 `mipmaps: bool`：`Image::decode` 解出的 PNG / JPEG 是 `true`；`from_rgba`、白色贴图、KTX2 是 `false`（文字的字形图集、程序生成的贴图不动）。
- `GpuImages`：
  - 上传 RGBA 贴图时，要 mipmap（`set_mipmaps` 的设置优先，没设就看 `Image::mipmaps`；最长边大于 1）就建完整的 mip 链（`⌊log2(最长边)⌋ + 1` 级），写第 0 级，再一级一级在 GPU 上生成：每级一个渲染通道，片元着色器 `textureLoad` 上一级的 2×2 个像素（奇数边丢掉最后一列 / 行，边长是 1 的那边重复用，和 KTX2 工具一样）。
  - sRGB：纹理是 `Rgba8UnormSrgb`，`textureLoad` 读出来已经是线性值，渲染目标也是 sRGB 视图，写回时再编码。所以求平均在线性空间里做，一黑一白缩出来是 sRGB 188 左右，不是 128。
  - 透明度：颜色按 alpha 加权平均（全透明的像素颜色不算进去），alpha 直接平均；四个都透明时颜色直接平均。透明边上不发黑。
  - 管线按目标格式缓存（sRGB / UNORM 各一条），第一次用时建。
  - 法线图：存的是方向数据，不是颜色。`nge-render2d` 的 `mark_normal_maps`（`PostUpdate`）把带 `NormalMap` 的贴图用 `set_linear` 标出来，这种贴图的 mip 链用 `Rgba8Unorm`（按存的值平均，不按 sRGB 解码），一黑一白缩出来是 128 左右。平均以后不重新归一化（见「不做」）。
  - 压缩贴图（KTX2）和回退解码都照旧用文件里的级，不生成、不改。
  - `set_mipmaps(id, on)` / `set_linear(id, on)`：按贴图设，可以在贴图加载好之前设。上传以后改了（结果和上传时不一样），下一次 `sync` 重传，`generation()` 加一；2D、3D 渲染器看到代数变了就丢掉缓存的绑定组重建。一样的不重传。
- `scene.textures: { 名字或 asset:/ 路径: { mipmaps } }`（`sync_textures`）：同步时解析成贴图（注册的纹理、图集的那张图，或者开始加载 `asset:/` 路径），调 `set_mipmaps`；从表里去掉的恢复默认（对还没上传的有用）。 几个名字指到同一张贴图（图集名和它的图、同一个路径的不同写法）又写得不一样时，按关掉算，并 warn。
- 采样器没改：mip 级之间取最近的一级（compressed-textures.md）。
- WebGL2 / Android（GLES）：生成 mip 的着色器有一个只编译不运行的检查（`the_mip_shader_translates_to_webgl2_glsl`：转成 GLSL ES 300）。真机上渲染到 sRGB 贴图的某一级、`textureLoad` 读上一级，还没在 WebGL2 和 Android 上跑过（见「还没手测」）。

### 平台跳跃

- 都在 `prefabs.ts`（预制体、过场设置、贴图设置），`game.tsx` 放进场景；`flow.test.tsx` 测 TSX 的部分。
- **过场**：场景写 `transition`，偶数关淡黑、奇数关擦除，淡出 0.35 秒、淡入 0.35 秒、最少 0.8 秒；`preload` 是金币和通关的音效。状态栏在有资源要等的过场里显示「载入中 3 / 5」（`transitions().loading()`；开场的揭开不显示「载入中 0 / 0」）。`onKey` 的数字键选关在过场里不响应（引擎挡住的是游戏的输入，`onKey` 是平台层直接发给 TSX 的）。
- **预制体**：远处的 24 棵树换成 12 个「树丛」实例：
  - `assets/tree.json`：树干加树冠（JSON 文件，用路径实例化）。
  - `prefabs.ts` 的 `grove`：两棵 `tree`（嵌套），第二棵用 `set` 把树冠抬高 14（原来单双号的树冠高度不一样）。
  - 树丛放在场景的 `prefabs` 里，`instances` 摆 12 个。树的贴图是纯色，不用等；`tree.json` 在第一次用到时才开始加载：第一关是从盖满揭开、不等预加载，树在载好的那一帧才出现（通常在揭开的那 0.35 秒里，被盖着）。之后换关时文件已经在内存里，当帧展开。
- **mipmap**：主角的图集 `hero.png` 和地面的瓦片集 `tiles.png` 关掉——都是像素风的一格格小图：mip 链按整张图算，缩小时相邻格子的颜色会混进来（格子边上出现杂色的缝），像素风缩小也该保持硬边。用图集、瓦片集的游戏都有这个风险，镜头会拉远的要么关掉，要么给格子留出血边。远山是 KTX2，自带 mip；3D 模型的贴图默认开（远处不闪）；石头的法线图 `stone_n.png` 按数据生成 mip。

## TSX 用法

类型和帮手在 `ts/flow.ts`，从 `ts/nge` 导出。

```tsx
import { gameView, preloader, transitions, type Prefab, type Scene } from "../../ts/nge";

const tree: Prefab = { sprites: [{ id: "trunk", y: 40, w: 10, h: 80 }, { id: "crown", y: 90, w: 44, rotation: 45 }] };

const loads = preloader(() => game);
const fade = transitions();

const scene = (): Scene => ({
  level: level(),
  prefabs: { tree },
  instances: [
    { id: "t1", prefab: "tree", x: 100 },
    { id: "t2", prefab: "asset:/@mygame/rock.json", x: 300, set: { rock: { color: "#888888" } } },
  ],
  transition: { kind: "wipe", out: 0.4, in: 0.4, min: 1, preload: ["asset:/@mygame/boss.wav"] },
  textures: { "asset:/@mygame/pixel-hero.png": { mipmaps: false } },
  sprites: [...],
});

// 提前加载下一章的资源，进度是 signal，ready 是 Promise
const { progress, ready } = loads.load(["asset:/@mygame/ch2.png", "asset:/@mygame/ch2.json"]);
ready.then((p) => p.failed.length && console.warn(p.failed));

<GameView scene={scene()} onPreload={loads.handle} onTransition={fade.handle} />;
// fade.busy()：过场中；fade.loading()：有资源在等（显示「载入中」用）；fade.progress()：{ done, total }
// 进度条用 load(...).fraction()（只涨不退）；视图重建时 loads.reset()
```

## 不做

- 3D 节点（模型、3D 灯、3D 粒子、3D 关节）放进预制体；实例的旋转和缩放；预制体里的覆盖用深合并。
- 预制体里引用改名只认上面列的那些字段（关节两头、`follow`、`chase.target`、行为树的目标）；别的写着 id 的地方（比如 `ai` 的导航网格引用、TSX 自己在命令里发的 id）不改。
- 预制体的实例在 Rust 里按组操作（一起移动、一起删）：展开后就是普通节点，按 id 前缀自己找。
- 重复的 id 自动改名：只 warn，同一个列表里后面的盖掉前面的。
- 一个 JSON 文件里放好几个预制体、预制体文件改了热重载。
- 第一次声明关卡时也等预加载：现在直接从盖满揭开，资源边揭开边到。
- 更多过场（圆形收缩、溶解）、过场用着色器画；过场里显示加载画面（TSX 用 `transition` 事件自己叠 NSH 的视图）。
- 盖子盖住文字：文字是另一个渲染器画的，排在精灵后面，盖子盖不住 `scene.texts`、气泡、飘字和性能面板。要盖住得把盖子挪到文字后面画（单独一个渲染器），这一轮没做；在意的游戏过场时自己把字去掉。
- 挡住平台层直接发给 TSX 的 `key` 事件、NSH 自己的按钮：TSX 看 `transitions().busy()` 自己挡。
- 过场超时以后再等：超时的资源记成失败、照样换关，之后载好了照常用（贴图冒出来）。
- 预加载的取消、优先级、并发数，卸载资源（句柄不计数，加载后一直留着）。`preload` 命令在 Rust 里没有超时（TSX 的 `preloader()` 自己有）。
- mipmap 用 compute 着色器一次生成多级、线性空间以外的滤波（Kaiser 等）；法线图的 mip 重新归一化（现在按存的值直接平均，斜的法线缩小后会短一点）；3D 模型的法线贴图（模型现在只用颜色贴图）。
- 压缩纹理没带 mip 时补生成；压缩纹理的 `set_mipmaps` / `set_linear`（照文件里的来，设了也不动）。
- 按 mip 级之间插值（三线性）：采样器还是取最近的一级。

## 验收

新加的测试：

- `nge-assets`：
  - `group.rs`：加载好的和失败的都算完成、带原因；同一个句柄只算一次；已经加载过的当时就完成、空组算完成；没注册的类型算失败不卡住。
  - `image.rs`：只有解码出的 PNG / JPEG 要 mipmap；mip 链一直减半到 1 像素。
  - `tests/mipmaps.rs`（真 GPU，没有 GPU 时跳过）：6×3 的图生成完整 mip 链，每级和 CPU 参考（线性空间、按 alpha 加权、边上重复）差不超过 2；黑白平均在 sRGB 是 186 … 190；透明的像素不把边染色；按贴图关掉、`Image` 自己不要、设置优先；程序生成的和白色贴图只有一级；KTX2 的级数不动、不补；上传后改开关不重传。
- `nge-ecs`：`actions.rs` 挡住输入时按着的键、摇杆、TSX 驱动的动作都读成没有，放开后要重新按。
- `nge-scene`：
  - `prefab.rs`：改名加前缀和平移；`set` 按子节点覆盖、在平移之前；嵌套、外层覆盖里层；里面的引用跟着改名、外面的不动、跟随的和屏幕上的不平移；循环和找不到的只跳过那一个实例；嵌套层数、节点总数有上限；文件还在加载时先不出；场景里的实例当帧出现、文件里的加载好以后展开、重新展开不重复；写错的实例和不是对象的预制体只丢那一个。
  - `preload.rs`：按扩展名认类型；场景列出它用到的资源（注册的纹理名不算、重复的一次）；失败的算完成、JSON 文件引用的资源也加进来、总数只变大、进度没变不重复发、完成后不再发；空组和已经加载过的当帧完成、完成的组扔掉。
  - `transition.rs`：没写过场时当帧换；盖的时候还是上一关、盖满同一帧换并发 `levelStart`、开场从盖满揭开、揭完不画；等预加载和最短时长；加载失败不卡住；整个过场挡住输入、开始那一帧就挡；盖的时候换回原来那关不重建；盖的时候场景又变了用最后一份；擦除的方向和大小（跟着相机、按缩放）；按住时相机也不动、放开后同步。
  - `sync.rs`：`scene.textures` 按名字、图集、路径设开关，去掉的恢复。
- TSX（`examples/platformer/flow.test.tsx`）：`preloader` 的命令、进度、ready、视图没建好；`transitions` 的 busy 和进度；树丛摆出来和以前 24 棵树一样；两种过场轮着用、主角不要 mipmap。
- 后来补的：
  - `nge-assets`：上传后改开关重传、代数加一，一样的不重传；法线图按存的值平均（上传前、上传后标都行）；`LoadGroup::pending`；生成 mip 的着色器能转成 WebGL2 的 GLSL（只编译）。
  - `nge-render2d`：带 `NormalMap` 的贴图标成数据。
  - `nge-scene`：实例扇出 8 宽 16 层按时停下；行为树的目标改名、`goto` 平移；用不到的 JSON 文件载好不重新展开；重复 id；按住时光照还是上一关的；按住时同一个认不出的 uri 只算一次；先空着再声明关卡也从盖满揭开；预加载前先设贴图开关；资源一直载不完时超时放开；按住时一直更新场景不重新计时；A → B → C 不等 B；字体、模型按自己的类型预加载；同一个 uri 只加一次；两个名字指同一张贴图时按关掉算。
  - TSX：`preloader` 没回、`reset()`、比例只涨不退；`transitions().loading()`；瓦片集也不要 mipmap。
- 平台跳跃（`src/lib.rs`）：`tree.json` 打包了，树丛实例按路径展开到以前第 3 棵树的位置。

全量检查（改完以后从头按顺序一个一个跑，都过了）：`cargo test --workspace -j 4`（1222 个）；`cargo test -p nge-scene --no-default-features -j 4`（206 个）；三个示例的每个 `*.test.tsx`（36 个文件、369 个）和 `nsh check`；`cargo check --target aarch64-apple-ios -p platformer -p sanguo -j 4`；`tools/check-wasm.sh -j 4`；`tools/check-android.sh`（只 aarch64）。

## 变异表

每一项：改坏、跑对应的测试、改回。

| 改坏的地方 | 挂掉的测试 |
|---|---|
| `gpu.rs` 求平均不按 alpha 加权 | `transparent_pixels_do_not_darken_the_edges`、`png_textures_get_a_full_mip_chain_matching_the_cpu_reference` |
| mip 链用 `Rgba8Unorm`（不按 sRGB 解码） | `averaging_happens_in_linear_space`、`png_textures_get_a_full_mip_chain_matching_the_cpu_reference` |
| `mip_levels` 少算一级 | `a_full_chain_halves_down_to_one_pixel` |
| 解码的 PNG 不要 mipmap | `only_decoded_pictures_want_mipmaps` |
| 上传时不看 `set_mipmaps` 的设置 | `mipmaps_can_be_turned_off_per_texture` |
| 着色器里不夹到最后一列 / 行 | **没挂**：等价变异，wgpu 对越界的 `textureLoad` 本来就夹到边上 |
| 上传后改开关不重传 | `changing_the_switch_after_the_upload_reuploads_and_bumps_the_generation`、`normal_maps_average_the_stored_values` |
| 重传时代数不加一 | `changing_the_switch_after_the_upload_reuploads_and_bumps_the_generation` |
| 法线图也用 sRGB 格式生成 mip / 不看 `set_linear` | `normal_maps_average_the_stored_values` |
| 不注册 `mark_normal_maps` | `normal_maps_are_marked_as_data` |
| `LoadGroup::pending` 列出已经完成的 | `pending_lists_what_is_still_loading` |
| `LoadGroup` 失败的不算完成 | `counts_loaded_and_failed_as_done` |
| `LoadGroup` 不去重 | `the_same_handle_counts_once` |
| 挡住输入时不 `release_all` / 不放掉 TSX 驱动的动作 / `release_all` 不清摇杆 | `blocked_input_reads_as_nothing_until_pressed_again` |
| 预制体不查循环 | `cycles_and_missing_prefabs_skip_only_that_instance` |
| 不限嵌套层数 / 不限节点总数 | `deep_or_huge_expansions_are_capped` |
| 先平移再覆盖 | `set_overrides_fields_by_child_id_before_moving`、`nested_prefabs_compose_and_outer_overrides_win` |
| 引用不管指向里面外面都加前缀 | `references_inside_the_prefab_follow_the_rename` |
| 跟着精灵的文字 / 进度条也平移 | `references_inside_the_prefab_follow_the_rename` |
| 重新展开前不截回原来的长度 | 一开始**没挂**（同步按 id 去重，实体数不变）；补了「场景里的精灵数不变」的断言后 `scene_instances_become_entities_and_file_prefabs_arrive_later` |
| 预制体文件加载失败当成还在加载 | **没挂**：两种都是先不出这个实例，差别只在 warn |
| 不是对象的预制体也留着 | `a_bad_instance_drops_only_itself` |
| `scene_ready` 不看按住 | `the_old_level_stays_until_covered_then_the_new_one_fades_in` 等 5 个 |
| 放开时不标记场景变了 | `sync_waits_while_held_but_runs_once_released` 等 5 个 |
| 放开不等预加载 | `failed_preloads_do_not_hold_the_switch` |
| 放开不等最短时长 | `the_switch_waits_for_preloads_and_the_minimum_time` |
| 放开不等盖满 | `the_old_level_stays_until_covered_then_the_new_one_fades_in` 等 4 个 |
| 不挡输入 | `input_is_blocked_for_the_whole_transition` |
| 换回原来那关时不放开 | `going_back_to_the_current_level_while_covering_cancels` |
| 擦除揭开时也盖左边 | `a_wipe_covers_from_the_left_and_uncovers_to_the_right` |
| 开场不从盖满揭开 | `a_wipe_covers_from_the_left_and_uncovers_to_the_right`、`the_old_level_stays_until_covered_then_the_new_one_fades_in` |
| 淡黑不乘 `cover` | `the_old_level_stays_until_covered_then_the_new_one_fades_in` |
| 盖子大小不除缩放 | `a_wipe_covers_from_the_left_and_uncovers_to_the_right` |
| JSON 文件引用的资源不加进组 | `progress_counts_failures_as_done_and_follows_json_files` |
| 完成的预加载组不扔 | 一开始**没挂**；补了断言后 `an_empty_or_cached_group_finishes_at_once` |
| 进度没变也发事件 | **没挂**：内存里的文件加载太快，测试里等不到一帧进度不变；补的「不重复发」断言留着 |
| `scene.textures` 去掉的不恢复 | `scene_textures_switch_mipmaps_by_name_or_path` |
| 展开实例不占上限（只有实例的扇出） | `wide_and_deep_chains_stop_in_time`（另一个线程里展开，5 秒没完就挂） |
| 行为树的 `chase` 目标不改名 / `goto` 不平移 | `behavior_targets_follow_the_rename_and_goto_moves` |
| 任何预制体文件载好都重新展开 | `unrelated_scene_files_do_not_reexpand` |
| 不找重复的 id | `duplicate_ids_are_found` |
| 光照不经同步、每帧读 `Scene` 的（不更新 `SceneLighting`） | `the_new_lighting_waits_for_the_switch` 和 effects 的 3 个 |
| `Preloading` 不按 uri 去重 | `the_same_uri_is_added_once`、`the_same_unknown_uri_counts_once_while_held` |
| 预加载把字体 / 模型当贴图载 | `fonts_and_models_load_as_their_own_kind` |
| 按住时每份新场景都重新开始计时（`!t.holding()` 改成 `true`） | `updates_while_held_do_not_restart_the_minimum_time` |
| 盖着的时候又换了一关不重开加载组 | `switching_again_while_covered_stops_waiting_for_the_middle_level` |
| 不超时 | `a_load_that_never_ends_times_out_and_lets_go` |
| 预加载前不先设贴图开关 | `scene_textures_apply_before_the_preload_uploads` |
| 之前没写 `level` 的场景之后第一次写，当成换关 | `the_first_level_fades_in_even_after_frames_without_one` |
| 两个名字指同一张贴图时按顺序后写的算 | `two_names_for_one_texture_turn_mipmaps_off` |
| TSX：比例按当前的算（会倒退） | `flow.test.tsx` 的比例测试 |
| TSX：开场揭开也算「载入中」 | `flow.test.tsx` 的「载入中」测试 |
| TSX：没回时失败的重复记 / 收到进度后不重新计时 | `flow.test.tsx` 的「没回」测试 |
| TSX：揭开时不算 busy | `flow.test.tsx` 的过场测试 |
| TSX：预加载完成后不删 | `flow.test.tsx` 的 load 测试 |
| TSX：揭开时不留载入进度 | `flow.test.tsx` 的过场测试 |
| 平台跳跃：树丛位置不加单双号的偏移 | `flow.test.tsx` 的树丛测试 |

## 修过的问题

- **只有实例的扇出卡死**：节点上限只管节点，一条 p0 … p15 的链每层放 8 个实例、不放节点时，要展开 8 的 16 次方次，`PreUpdate` 卡住（7 层就要两秒多）。现在每展开一个实例也占上限，用完了就停。
- **按住时新关卡的光照照在上一关上**：`lighting` 每帧直接读 `Scene`，按住新场景时已经按新关卡摆灯（白天换到夜关时，上一关先变暗）。改成读同步时存下的那份。查过别的每帧跑的系统都不读 `Scene`。
- **预加载过的贴图不认 `scene.textures`**：过场或 `preload` 先把贴图载好，渲染器当帧就按默认上传了，场景里的 mipmap 开关之后才设，又不重传。现在开始预加载前先设开关；而且改了开关会重传（渲染器按代数丢掉旧的绑定组）。
- **法线图的 mip 按颜色平均**：法线图也是 sRGB 贴图，生成 mip 时先按 sRGB 解码再平均，方向被拉偏（`stone_n.png`）。现在带 `NormalMap` 的贴图按存的值生成。
- **资源一直载不完时过场放不开**：输入一直挡着，联网也停着。加了 `timeout`（默认 10 秒），到了把没载完的记成失败、照样换关。
- **按住时总数一直涨**：按住期间每来一份场景，认不出类型的 uri 就再记一次失败。现在同一组里按 uri 去重。
- **先空着再写关卡时淡出**：只有「第一次见到场景」才当开场；先来一份没写 `level` 的场景、再写关卡时，被当成换关淡出。现在之前没写过 `level` 都算开场，从盖满揭开。
- **盖着的时候连换两关还等中间那关**：A → B → C 时 B 的资源还在同一组里，要等它载完。现在换了目标就重开一组。
- **行为树的目标不跟着改名**：预制体里精灵的行为树 `see` / `near` / `chase` 指向预制体里的节点时没加实例前缀，`goto` 也没平移。
- **用不到的预制体文件也触发重新展开**：任何 JSON 文件（比如 `preload` 载的关卡文件）载好都重新展开、整份场景重新同步一遍。现在只认这个场景展开时找过的文件。
- **两个名字指同一张贴图时结果看运气**：图集名和路径写了相反的值，谁算数看哈希表的顺序。现在按关掉算并 warn。
- **重复的 id 不说**：实例 id 重复、展开出来的和手写的撞了，同一个列表里后面的悄悄盖掉前面的。现在 warn。
- **开场的揭开显示「载入中 0 / 0」**：平台跳跃的状态栏只看过场里不在。现在看 `transitions().loading()`（有资源要等才算）。
- **TSX 的预加载可能一直挂着**：引擎不回、视图重建时 `ready` 永远不完成。现在 30 秒没回按「没回」结束，`reset()` 手动结束。
- **TSX 的进度条会倒退**：JSON 文件载好后总数变大。加了只涨不退的 `fraction()`。
- **瓦片集缩小渗色**：平台跳跃的 `tiles.png` 也是一格格的小图，现在也关掉 mipmap。

- 还没手测：Mac 上看过场的样子（淡黑、擦除）、换关时有没有资源冒出来、过场里按键不响应、树和以前一样、主角和地面缩小时的样子、石头法线图缩小后的光照；iPhone 上的过场和 mipmap。
- 还没验证：WebGL2 和 Android 上生成 mip（只检查了着色器能转成 GLSL ES 300、能编过 wasm32 和 aarch64-linux-android）；改开关重传后渲染器丢掉旧绑定组（代数有测试，渲染器那一步没有 GPU 测试）。
