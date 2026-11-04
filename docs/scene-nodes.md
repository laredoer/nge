# TSX 场景节点：3D、文字、粒子

> 前置：M3（TSX 声明式场景）、M4（3D）、M5（资源层、文字、粒子）。

## 目标

TSX 里除了精灵，还能声明 3D 模型、文字和粒子，并触发一次性效果；资源直接写 `asset:/` 路径，Rust 侧只建 App、加插件。

**验收标准**：新示例 `examples/scene3d`，场景全部在 `App.tsx` 里：
- 地面上几个 glTF 箱子在转，头顶有名字牌（跟着箱子在屏幕上的位置走）；
- 一根「烟囱」一直冒粒子；
- 点中箱子：那里炸一团粒子、飘出「+10」，HUD 上分数增加；点空地没反应；
- 贴图、字体、模型都在节点里写 `asset:/` 路径。

## 两类东西

- **持续存在的**放进 `scene` prop，和精灵一样按 id 增量同步：节点没变就不碰实体，变了整体覆盖，删掉就删实体。
- **一次性的**走 command，引擎播完自己清掉：`burst`（炸一团粒子）、`floatText`（飘字）。TS 里用 `burst(node, …)` / `floatText(node, …)` 发，`node` 是 `<GameView ref>` 拿到的节点。预设（「爆炸长什么样」）由游戏在 TS 里自己写成常量对象，引擎不做命名预设。

## `scene` 新增的字段

每种节点各用各的 id 空间；Rust 侧按 `SceneId` 找实体时要加 `With<Sprite>` 之类的过滤，不然会同时找到同名的文字或发射器。坐标约定和现在一致：2D 是世界坐标（point，y 朝上，原点在视口中心），角度用度；3D 是右手系、y 朝上。

| 字段 | 内容 |
|---|---|
| `sprites[].texture` | 现有的注册名之外，也可以写 `asset:/…`；配 `grid: [cols, rows]` 当图集 |
| `texts[]` | `{ id, text, x, y, size=16, color, anchor="center", font, screen=false, maxWidth, follow, lift=0 }`。`font` 是 `asset:/` 路径，不填用系统字体。`screen` 为真时 x / y 是屏幕坐标（左上角原点，y 朝下）。`follow` 填一个模型的 id：文字跟着那个模型投影到屏幕的位置走，`lift` 是往上抬的 3D 距离，x / y 变成屏幕上的偏移 |
| `emitters[]` | `{ id, x, y, z, rate（默认 0，不发射）, lifetime: [min, max], speed: [min, max], angle: [min, max]（度）, gravity: [x, y], size: [出生, 死亡], color: [出生, 死亡], texture, max }`，持续发射，不会自动删除 |
| `models[]` | `{ id, model, x, y, z, rotation: [x, y, z]（度，按 Y、X、Z 的顺序转）, scale, spin（绕世界 y 轴，度 / 秒）, tap }`，`model` 是 `.glb` / `.gltf` 的 `asset:/` 路径 |
| `camera3d` | `{ x, y, z, target: [x, y, z], fov（度）}`；不填时在 (0, 2, 6) 看向原点 |
| `lights[]` | `{ id, kind: "directional" \| "point", direction: [x, y, z], x, y, z, color, intensity, range }` |
| `ambient` | `{ color, brightness }` |

一次性命令的参数：
- `burst`：`emitters[]` 去掉 `id` 和 `rate`，加 `count`（默认 100），粒子死完自动删除；
- `floatText`：`{ x, y, text, size, color, font, duration=1, rise=60 }`，世界坐标，往上飘、渐隐。

## 点击

`tap` 事件照旧是 `{ id, x, y }`（x / y 是指针的 2D 世界坐标，可以直接拿去 `burst`），点中模型时多一个 `point: [x, y, z]`（3D 命中点）。精灵画在 3D 上面，所以先判断可点的精灵，没点中再对 `tap: true` 的模型做射线拾取：用模型所有网格的包围盒（模型本地空间），取最近的一个。不可点的精灵不挡点击（和 2D 一样），点穿过去还能打到后面的模型。

## 实现上的决定

- `ScenePlugin` 要求先加好 `Render3dPlugin`、`Render2dPlugin`（先 3D 后 2D，顺序反了模型会盖住精灵，插件查不出来）；`ParticlePlugin`、`TextPlugin` 没加时自己补上。`App::add_plugins` 同一种插件只 build 一次，之后再加只记 warn。场景只有 2D 时 3D 渲染器只清屏，代价可以忽略。
- 场景自己放一个 `Camera3d`（带 `SceneCamera` 标记），只在 `camera3d` 声明变了时覆盖；`ambient` 单独比较，改环境光不会把 Rust 移动过的相机拉回去。视线和 y 轴平行（正上方往下看）时改用 -z 当 up。
- `grid` 的格数超过 65536 时当成整张图，避免格数相乘溢出。
- 资源 uri 每次同步都交给 `AssetServer::load`（它自己去重）；`asset:/` 贴图 + `grid` 的图集按 (uri, grid) 缓存。
- 模型节点换了 `model` 时，拿掉已展开的 `Model3d` 再挂新的 `GltfModel`。
- 名字牌每帧在 PostUpdate 里投影：模型在相机后面时把文字挪出屏幕。
- 飘字做成 `nge-text` 里的通用组件 `FloatingText`，`examples/effects` 改用它。
- 已注册的纹理名（`ScenePlugin::texture` / `atlas`）保留，现有的 `examples/scene` 不用改 TSX。

## 范围外

- 物理、音频、TSX 里写每帧逻辑
- 3D 粒子、贴在 3D 空间里的文字
- 拾取只用包围盒，不精确到三角形

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-scene` 36 个（新增：文字的屏幕坐标和跟随、发射器只移动时保留已有粒子、`asset:/` 贴图和按 (uri, grid) 缓存的图集、模型的变换 / 旋转 / 换模型时重新加载、灯光 / 3D 相机 / 环境光、正上方往下看的相机、改环境光不重置相机、过大的 `grid`、两个模型前后重叠时点中近的那个、精灵优先于模型且不可点的模型被忽略、射线打不到身后的东西、名字牌投影和模型在相机后面时挪出屏幕、`burst` / `floatText` 命令）；`nge-ecs` 新增插件记录和重复添加只 build 一次 2 个；`nge-text` 新增 `FloatingText` 1 个；`examples/scene3d` 2 个（用和 `App.tsx` 一样的 JSON：点中间的箱子得到 `{ id: "green", point }`、点天空没有事件；`burst` / `floatText` 的参数格式）。
- **TS**：`nsh check`（nsh 0.3，TypeScript 7）通过 `examples/scene3d` 和 `examples/scene`；故意写错一个字段类型时能报出来。
- **离屏截图**：用临时测试把 `scene3d::factory()` 渲染成 PNG：三个箱子在转、名字牌在各自头顶、烟囱冒烟、HUD 提示文字、中间那团爆炸和「+10」都画出来了。
- **实跑**：`nsh dev --server-only` + `cargo run -p scene3d --release`，dev server 显示 macOS host 已连接，日志里没有资源加载失败的 warn。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过；没上真机（没连手机）。

## 已知限制 / 后续

- `nsh dev` 不托管游戏 crate 的 `assets/`（`/assets/@scene3d/…` 返回 404，因为 `nsh.json` 没声明 Rust crate），`cargo run` 时靠回退到 `assets!()` 记下的源文件路径；真机走 dev server 时要另想办法。
- 拾取只用模型本地空间的包围盒，转着的长条模型在包围盒的空角上也算点中。
- 名字牌按模型原点 + `lift` 投影，不考虑模型的缩放。
- `scene` 里任何一个字段解析失败（如 `kind: "spot"`），整份声明都不生效，只记一条 warn，TSX 收不到错误事件（M3 起就是这样）。
- 每次点击都现算可点模型的包围盒（遍历顶点），名字牌每帧线性找模型；模型多、面数高时再缓存。
