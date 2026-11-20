# 大场景性能：瓦片分块裁剪和局部重建、视锥裁剪、GPU 时间、性能曲线

> 前置：瓦片地图（[tilemap.md](tilemap.md)）、2D 玩法补齐（[gameplay2d.md](gameplay2d.md)）、性能面板（[perf-overlay.md](perf-overlay.md)）、3D 画质（[render3d-quality.md](render3d-quality.md)）、3D 粒子补齐（[particles3d-more.md](particles3d-more.md)）。

## 目标

现在的瓦片地图每格一个精灵实体，地图节点任何字段变了整张重建（格子精灵、碰撞体、导航网格全部重新生成）。2000 × 200 的地图是四十万个实体：建一次要好几秒，改一格（挖砖、编辑器里刷一下）也是整张重来，每帧渲染还要把四十万个精灵全部排序、上传。精灵、3D 网格也不看相机，画面外的照样画。性能面板只有 CPU 的平均数，看不出 GPU 花了多久，也看不出哪一帧卡了。

**验收标准**：
- 瓦片地图：
  - 按块（默认 32 × 32 格）存，每块各自缓存顶点；只画视野（加一格外扩）里的块和格子，镜头移动时只给新进来的块建缓存，出去久了的块放掉缓存；
  - 裁剪按每个相机的视野算（多个视野时取并集），视差层、旋转、缩放的镜头都裁得对；
  - 改一格只重建那一格所在的块：画面的缓存只重建那一块，碰撞体只重建那一组（见下）而且没变的碰撞体不换实体，导航网格、地形只改那一格；
  - 动画瓦片不重建：按场景时钟算出当前帧，只改可见块里那些格子的 uv；
  - 走地面的寻路图做不到局部：地形变了在后台线程重建（Web 上节流，最多半秒一次），建好之前用旧图；
  - TSX 改地图声明里的 `data`，或者发 `setTiles` command，都走局部更新。
- 2D 精灵按相机视野裁剪；3D 网格按相机视锥和世界包围盒裁剪（阴影 pass 原来就按光的视锥裁）；整个看不见的 2D / 3D 粒子发射器跳过打包和上传。
- GPU 时间：设备支持时用时间戳查询量各个 pass 的 GPU 耗时，结果异步读回、不阻塞帧；不支持（WebGPU 没开、部分手机）时面板写「GPU 不支持」，不崩。
- 性能面板：
  - 加一行 GPU 时间、一行裁剪统计（瓦片块、精灵、网格各画了几个 / 一共几个，画了多少格瓦片）；
  - 可以打开滚动曲线（最近 120 帧）：帧时间、CPU 分段（更新、渲染、等待各一条线）、GPU 时间、实体数、draw call 数；用方片画，一帧几百个实例；
  - TSX 可以收 `perf` 事件，每半秒一份汇总。
- 性能回归测试：2000 × 200 的地图用 `setTiles` 改一格、改声明里的一格、镜头移动的一整帧，CPU 耗时都在阈值以下。
- 平台跳跃：按 B 进出「大地图」（2000 × 80，点格子挖掉，U 填回去），按 O 开关曲线。

## 做法

### 瓦片分块（`nge-render2d` 的 `tiles.rs`）

- 组件 `TileLayer`：一张地图的全部格子（帧号）、左上角、格子边长、z、视差、块的边长（格数）、动画瓦片，和每块的缓存。地图实体自己挂它，不再给每格生成精灵实体。
- 块按行优先编号，块 `(bx, by)` 管 `[bx × chunk, (bx + 1) × chunk) × [by × chunk, …)` 的格子，地图右边、下边不满一块的照样一块。
- 每块的缓存：这一块非空格子的实例（和精灵一样的实例，uv 已经算好）。缓存在渲染时按需建：
  - 块是脏的（改过格子）或者图集的贴图换了（加载完、白图换成真图）时整块重建，`rebuilt()` 加一；
  - 动画瓦片：按场景时钟（`Time::elapsed`，暂停、时间缩放都跟着）算出当前帧 `frames[⌊t × fps⌋ mod n]`，和块里记着的上一次不一样时只改那些格子的 uv，`patched()` 加改了几个；
  - 一块连续 `KEEP_FRAMES`（120）帧都不可见就放掉缓存，内存不随走过的距离一直涨。
- 可见判断（`View2d`：相机加视口大小）：
  - 视差偏移和精灵一样加在地图上（见 [parallax.md](parallax.md)）；
  - 视野是转过的矩形：先用它的外接框加外扩（一格）算出可能相交的块的行列范围，再对每块做分离轴测试，转着的相机不会把外接框角上的块算进来；
  - 块里只拷视野外接框加外扩以内的格子，不是整块全画；
  - `visible(views, …)` 收多个视野、块取并集。引擎现在只用第一个 `Camera2d` 画（见「不做」）。
- 渲染器（`renderer.rs` 的 `draw_tiles`）：背景层、前景层各自按 `z` 取地图，拷可见的实例进合批，和精灵一起按 z、贴图排序；一张地图的瓦片同 z 同贴图，合成一次 draw。

### 场景里的地图（`nge-scene` 的 `tilemap.rs`、`tile_chunks.rs`）

- 地图实体：`TileLayer`、`TileBodies`（碰撞体，按组记）、`NavGrid`、`GroundMap`、`TileTap`（有 `tap` 时，只是个标记，点中哪一格、什么帧都从 `TileLayer` 读）。
- 碰撞体按「条」建，一条是 `chunk` 行高、整张地图宽：
  - 横着不切开，所以平地还是一整块，角色横着走不会撞到接缝。没用正方形的块是为了不在地面上切出竖的接缝；
  - 斜坡跨过两条之间的接缝（一条坡上相邻的两格分在接缝上下，`tile_slopes::crosses`）时，这两条合成一组一起建，坡不会被切开，也不会多出一块尖角顶在坡面上的实心矩形。一条斜着跨好多条的长坡会把这些条都并成一组；
  - 每组只看自己的格子：斜坡、实心、单向平台的合并规则和原来一样，只是不跨组。剩下的接缝只在没有坡跨过的条之间，是一条横线：竖着的墙、坑壁上下分两段，从侧面贴着滑没有影响。
- 局部更新：
  - 声明里只有 `data` 变了（长度和别的字段都一样，`only_cells_changed`）时，按新旧声明逐格比，只改变了的格子；
  - `setTiles` command（`{ id, tiles: [[列, 行, 帧号], …] }`）直接改格子，不经过声明；地图不存在、地图本身无效（`cols` 是 0、`tile` 不是正数，没建出来）、格子在地图外、帧号太大的那一项 warn 后跳过，提示分得清是哪一种。之后声明的 `data` 再变时只改声明里变了的格子，`setTiles` 改过、声明里没变的格子不会被改回去；规则变了整张重建时回到声明的样子；
  - 同一帧里声明和 `setTiles` 改了同一格时 `setTiles` 胜：声明在 PreUpdate 同步，`setTiles` 在 Update 里改；
  - 改格子（`edit_map`）：`TileLayer::set`（只标脏那一块）；`NavGrid`、`GroundMap` 只在那一格真的变了时才改（只读不改时不标「改过」，装饰帧之间换、全部越界的 `setTiles` 都不会让寻路图重建）；
  - 碰撞体只重建改到的条所在的组。改到条的第一行、最后一行时重新看那条接缝有没有坡跨过，组连上、断开时新旧两种分组涉及的条一起重建。重建时新算的碰撞体和旧的按（位置、形状）比，一样的留着原来的实体：物理的句柄不变，静止压在上面的物体不会收到多余的 `collide` stopped / started，正在往下穿的单向板（`Physics::dropping` 按句柄记）也还认得。`TileBodies::rebuilt()` 记着重建了几组。
- 碰撞事件不变：碰撞体还是地图的子实体，带 `TileSolid`。
- 走地面的寻路图（`ground_graphs.rs`，`behavior.rs` 用）：做不到局部。键是（地图、角色的跳跃能力和大小、地图的左上角和行列数）。
  - 整张重建（`GroundMap` 是新插进来的）或者地图挪了、变大小了：旧图不要，用到时当场建；
  - 只改了格子：先接着用旧图，原生平台同时在后台线程按新地形建一份（`std::thread` + 通道，主线程只克隆一份地形，2000 × 200 大约 0.6 ms），建好了下一帧换上；建的时候地形又变了，换上以后再建一次。Web 上没有线程，当场建，但一张图最多半秒（`GRAPH_EVERY`）建一次，中间先用旧图；2000 × 200 建一次大约 16 ms（原生 debug），Web 上那一帧会卡一下；
  - 地图删掉时它的图也删掉。
- 内存（2000 × 200，40 万格）：`TileLayer` 的帧号 4 字节一格（1.6 MB），`NavGrid` 1 字节一格（0.4 MB），`GroundMap` 的地形 12 字节一格（4.8 MB），声明（`Scene` 和 `Declared<TilemapNode>` 各一份 `data`）各 1.6 MB；块的缓存只有看得见、和最近 120 帧见过的块才有。点地图不再另存一份。

### 精灵、网格、粒子的裁剪

- 2D 精灵：按画的位置、转过以后的半宽高和视野做同样的分离轴测试，看不见的不进合批。用自定义着色器的精灵不裁（着色器可能把顶点挪出包围盒）。统计按精灵数记 `[画了, 一共]`。
- 3D 网格：收集完以后按世界包围盒和相机的 `view_proj` 判断（`in_view`，8 个角全在同一个裁剪面外才算看不见；包围盒坏了的照画），看不见的主 pass 不画。阴影 pass 原来就按光的视锥裁，投影子的照样按全部不透明的算，画面外的东西影子还能落进画面。
- 粒子：2D 发射器把粒子的位置框（加最大的大小）和视野比，3D 发射器（广告牌）和相机视锥比；整个看不见时不往实例里放，模拟照常（子发射器、碰撞、死亡都不受影响）。粒子模拟排在相机跟随（`camera_rig`、`match_2d`）之后，用的是这一帧挪完的相机；性能曲线（`PerfDrawn`）也一样，不会落后一帧。

### GPU 时间（`nge-ecs` 的 `gpu_time.rs`）

- 建设备时适配器支持 `TIMESTAMP_QUERY` 就要上（`nge-core` 的 `optional_features`）。
- 资源 `GpuClock`：设备没有这个特性时 `supported()` 是 `false`，什么也不量。有时建一个 128 个查询（64 个 pass，`MAX_PASSES`）的 `QuerySet`、一个解析缓冲和 3 个读回缓冲（轮着用）。
- 一帧开头（`begin`）：`poll` 一下（不等），收已经映射好的读回缓冲；几帧同时好了时只读帧号最新的那一帧，别的直接解除映射。算出每段毫秒（`(结束 − 开始) × timestamp_period`，结束比开始小的当 0）。挑一个空着的读回缓冲给这一帧，都在用时这一帧不量。
- 渲染器开 pass 前调 `nge_ecs::pass_timer(world, "精灵")` 拿计时器，开 pass 时 `timestamp_writes: timer.as_ref().and_then(PassTimer::writes)`。查询的下标在 `writes` 时才分：拿了计时器没开 pass 的（后处理里着色器没编好跳过的那一遍、bloom 里的几遍）不占位置，帧末只解析真写了的，不会解析没写过的查询。
- 一帧超过 64 个 pass 时多出的不量，这一帧记成「不全」：面板写「GPU ≥ x ms」，`perf` 事件的 `gpuTruncated` 是 `true`。阴影多的场景每一遍阴影一个 pass，64 个够用；不够时总时间是下限。
- 量了的 pass：精灵（背景、前景两层）、网格、软粒子、调试线、阴影（每一遍）、文字、后处理（每个效果一遍；bloom 里面的几遍不单独量）。
- 一帧画完（`end`）：把用到的查询解析进解析缓冲，拷进这一帧的读回缓冲，`map_buffer_on_submit` 在提交以后映射，回调里只改一个原子标记。一直不阻塞帧。
- `Perf` 的汇总带上窗口里读回的 GPU 时间的平均；不支持时 `gpu_supported` 是 `false`，还没读回时 `gpu_ms` 是空的。
- 精度：Metal、D3D12 上是纳秒级；WebGPU 默认把时间戳量化到 100 µs（浏览器防侧信道，开了 `chrome://flags` 的 WebGPU Developer Features 才是原值），小 pass 会显示 0 或 0.1 ms；Vulkan 上队列的 `timestampValidBits` 小于 64 时时间戳会回绕，回绕的那一帧结束比开始小，按 0 算。

### 性能面板、曲线（`nge-ecs` 的 `perf.rs`、`render.rs`，`nge-text` 的 `perf.rs`、`perf_graph.rs`）

- `RenderFrame` 里的 `CullStats`：渲染器记下瓦片块、精灵、网格各画了几个、一共几个，瓦片记画了几格；汇总取最后一帧的。
- 面板变成四行：
  ```
  60 fps  帧 16.8 ms（最长 33.3）
  更新 1.2  渲染 2.5  等待 13.0 ms  GPU 4.3 ms      ← 不支持时「GPU 不支持」，还没读回时「GPU 统计中」
  draw 42  实例 1830  实体 312  粒子 950
  块 6/250  瓦片 1400  精灵 20/300  网格 8/40
  ```
- 实体数按 archetype 加起来（不数每个实体），去掉资源实体。
- 资源 `PerfHistory`：最近 120 帧的帧时间、更新、渲染、等待、GPU（没有时空着）、实体数、draw call 数，环形缓冲，每帧记一笔。
- 资源 `PerfGraph(bool)`：开着时在面板下面画两张 240 × 48 的图，白色方块拼线段（精灵合批，`GIZMO_Z` 以上），一帧几百个实例：
  - 上面：帧、更新、渲染、等待、GPU 五条耗时曲线，满格按最大值取 16.7、33.3、50、100 … 2000 ms 里的一档；GPU 没有数时线断开；
  - 下面：实体数、draw call 两条，满格取 1、2、5 × 10ⁿ；
  - 下面一行图例写颜色和满格。屏幕坐标按相机换到世界（转着、缩放的相机也贴在屏幕上）。

### 场景和 TSX

- `TilemapNode.chunk?: number`：块的边长（格），默认 32，夹到 4 … 256；写错（不是数）时 warn、按默认（`lenient.rs` 的 `tile_chunk`）。
- `setTiles(game, { id, tiles: [[col, row, frame], …] })`：见上。
- `scene.perf` 可以是布尔（和原来一样，只管面板），也可以是对象 `{ panel?, graph?, events? }`，逐字段宽松解析（`Fields`），写错的字段 warn 后当没写，整个写错当没写；声明变了才写资源（Rust 自己开关的不被每帧覆盖），去掉声明时都关掉。
- `events: true` 时每出一份汇总发一次 `perf` 事件（`PerfEvent`）：`{ fps, frameMs, worstMs, updateMs, renderMs, waitMs, gpuMs, gpuSupported, draws, instances, entities, particles, chunks: [画了, 一共], tiles, sprites: [画了, 一共], meshes: [画了, 一共] }`，`gpuMs` 不支持或还没读回时是 `null`。

```tsx
const scene: Scene = {
  perf: { panel: true, graph: true, events: true },
  tilemaps: [{ id: "big", texture: "tiles", tile: 32, cols: 2000, data, solid: [0, 1], chunk: 32, tap: true }],
};
<GameView scene={scene} onTap={(t) => t.cell && setTiles(game, { id: "big", tiles: [[t.cell[0], t.cell[1], -1]] })} onPerf={(p) => console.log(p.frameMs, p.chunks)} />
```

### 演示（平台跳跃）

- 按 B（桌面）进出「大地图」（`bigmap.ts`）：2000 × 80 格，几段正弦叠起来的起伏地面、每 150 列一个水坑（动画瓦片）、每 40 列一块单向木板，开着面板和曲线，状态栏写操作。
- 点地面一格挖掉（`setTiles`），U 填回最近挖的一格；出来时这一关从头开始。
- 任何关里按 O 开关曲线（Rust 的 `toggle_graph`），P 照旧开关面板。

## 不做

- 多个 `Camera2d`：引擎只用第一个 `Camera2d` 画，瓦片、精灵的裁剪也只看它。`tiles::visible` 能收多个视野，以后加分屏、小地图时再接。
- TSX 每次都把整张 `data` 发过来：`<Tilemap data={…}>` 的 `data` 变了，整个场景声明还是整份序列化、整份解析（2000 × 200 一次十几毫秒，见「数字」）。引擎这边只是比较以后只改变了的格子，省不掉传输和解析。大地图建议 `data` 只在开头给一次，之后用 `setTiles` 改。
- `setTiles` 改的格子不回写声明：TSX 里的 `data` 还是旧的，存档、联网同步要的话由游戏自己记（比如把发出去的 `setTiles` 也记一份）。规则变了整张重建时回到声明的样子。
- 接缝：碰撞体只在没有坡跨过的两条之间切开，切口是一条横线。竖墙、坑壁在接缝处上下分两段，贴着墙竖着滑的物体可能在接缝处碰到两块碰撞体的角（`a_box_slides_down_a_slope_the_same_whatever_the_chunk` 只测了坡）。
- 走地面的寻路图做不到局部重建，见上面「场景里的地图」。
- GPU 时间只量 pass，不量 pass 之间的拷贝、清屏；bloom 里面的几遍算在后处理那一段里不细分。

## 变异表

每一项改坏一处、跑对应的测试、确认挂了，再改回来。

| 改了什么 | 哪个测试报错 |
| --- | --- |
| `View2d::overlaps` 只用外接框，不测视野的两个轴 | `tiles::a_turned_camera_skips_the_corners_of_its_bounding_box` |
| 看不见的块永远不放掉缓存 | `tiles::caches_out_of_sight_for_a_while_are_dropped` |
| 改格子时所有条的碰撞体都重建 | `set_tiles_rebuilds_only_the_band_it_touches`、`set_tiles_skips_bad_cells_and_unknown_maps` |
| `only_cells_changed` 总是假（只改格子也整张重建） | `local_edits_build_the_same_colliders_as_a_fresh_map` |
| `in_view` 总是真（3D 网格不裁） | `only_boxes_in_the_frustum_are_drawn` |
| 2D 发射器看不见也放实例 | `emitter::emitters_outside_the_view_still_simulate_but_are_not_drawn` |
| 3D 发射器看不见也放实例 | `emitter3d::emitters_outside_the_view_still_simulate_but_are_not_drawn` |
| 精灵不裁 | `offscreen::sprites_out_of_view_are_culled_but_counted` |
| `span_ms` 结束比开始小时回绕 | `spans_turn_ticks_into_milliseconds_and_never_go_negative` |
| `PerfHistory` 满了不丢最旧的 | `history_keeps_the_last_frames_oldest_first` |
| 实体数算上资源实体 | `entity_count_skips_resources_and_disabled_entities_and_follows_despawns`、`timing_goes_into_perf_with_this_frames_draws_and_entities` |
| `sync_perf` 每帧按声明覆盖 | `perf_panel_follows_the_scene_only_when_the_declaration_changes` |
| 地图改了当场重建寻路图 | `a_changed_map_keeps_the_old_graph_until_the_new_one_is_ready` |
| 重建时从不留旧的碰撞体实体 | `far_edits_in_the_same_band_send_no_collide`、`dropping_through_stacked_planks_survives_a_far_edit`、`set_tiles_rebuilds_only_the_band_it_touches` |
| 接缝永远当没坡跨过 | `slopes_across_seams_build_the_same_colliders_as_one_band`、`edits_that_join_or_split_a_slope_across_a_seam_match_a_fresh_map`、`a_box_slides_down_a_slope_the_same_whatever_the_chunk` |
| `crosses` 只看往下连的坡 | `slopes_across_seams_build_the_same_colliders_as_one_band`、`edits_that_join_or_split_a_slope_across_a_seam_match_a_fresh_map` |
| 改到条的第一行 / 最后一行时不重看接缝 | `edits_that_join_or_split_a_slope_across_a_seam_match_a_fresh_map` |
| 重建时不管旧的分组 | `edits_that_join_or_split_a_slope_across_a_seam_match_a_fresh_map` |
| `NavGrid` / `GroundMap` 不管变没变都改 | `edits_that_leave_the_terrain_alone_do_not_touch_nav_or_ground` |
| 斜坡、实心块的位置不加这一条的起始行 | `a_band_below_the_first_puts_its_colliders_at_their_rows` |
| `only_cells_changed` 不比 `chunk` | `every_field_but_data_rebuilds_the_whole_map` |
| 无效地图的 `setTiles` 提示不分开 | `set_tiles_on_an_invalid_map_changes_nothing` |
| 整张重建的地图还用旧寻路图 | `a_rebuilt_map_gets_its_graph_built_right_away` |
| 地图挪了、变大小时留着旧寻路图 | `a_map_that_moved_or_resized_drops_its_old_graph` |
| `sync_perf` 第一帧总按声明写开关 | `a_panel_rust_opened_survives_a_first_scene_without_perf` |
| 粒子不排在 `camera_rig` 后面 | `particles_are_culled_against_the_camera_after_it_moved_this_frame` |
| 几帧同时读回时取最旧的 | `the_newest_of_several_read_backs_wins` |
| 超过 64 个 pass 不标「不全」 | `passes_over_the_limit_are_dropped_and_flagged` |
| 同 z 时瓦片不排在精灵前 | `tiles_go_under_sprites_of_the_same_z_whatever_the_texture` |
| 瓦片可见范围右边往下取整 | `a_tile_one_cell_past_the_edge_is_drawn_two_cells_past_is_not`、`only_tiles_near_the_view_are_drawn` |
| 粒子的框不算大小 | `particles_just_past_the_edge_still_show_their_size` |
| 面板不写「≥」 | `lines_show_every_number` |

## 数字

2000 × 200 的地图，Mac（Apple Silicon，`cargo test` 的 profile）。「改之前」是这个分支开始前的代码（每格一个精灵子实体），同一个测试在那份代码上跑。

| 量什么 | 改之前 | 改之后 |
| --- | --- | --- |
| 整张建一次（一帧，取多次里最快的） | 48.8 ms | 16.1 ms |
| `setTiles` 改一格那一帧（取最快） | — | 2.1 ms |
| 声明里改一格（解析整份场景 JSON、同步、一帧，取最快） | 8313.6 ms（见下） | 17.0 ms |
| TSX 那边拼、序列化整份场景（用 `serde_json` 量） | — | 15.3 ms |
| 什么都不改的一帧 | — | 0.23 ms |
| 镜头移动的一整帧（更新 + 精灵渲染器录制、提交，1280 × 720，120 帧的中位数） | 12.07 ms（更新 5.01、渲染 6.74） | 0.60 ms（更新 0.31、渲染 0.27） |
| 镜头移动一步拷瓦片（`TileLayer::draw`） | — | 12 µs（3495 格） |
| 跳到新区域（建 12 块缓存） | — | 133 µs |

- 声明里改一格「改之前」的 8313.6 ms：老代码 `data` 一变就把 40 万个格子精灵子实体全删掉再建一遍，删实体那一步占了绝大部分。
- 声明里改一格「改之后」的 17 ms 几乎都是解析整份 JSON；同一帧里引擎自己的活和 `setTiles` 一样只有两三毫秒。所以大地图建议用 `setTiles`（见「不做」）。
- 镜头移动「改之前」「改之后」是同一个测试（`tests/big_map_perf.rs`）在两份代码上跑的，量的都是整帧在 CPU 上的时间，不含 GPU 画的时间。

回归测试的阈值：`setTiles` 改一格 60 ms（`editing_one_cell_of_a_huge_map_stays_cheap`），声明里改一格 60 ms（`declaring_one_changed_cell_of_a_huge_map_stays_cheap`），镜头移动拷瓦片 4 ms（`moving_the_camera_over_a_huge_map_copies_only_what_is_in_view`），镜头移动的整帧 5 ms（`moving_the_camera_over_a_huge_map_stays_cheap`，要 GPU，没有时跳过）。

## 验收

- 还没手测：大地图在 Mac、iPhone、Android 上的帧率，曲线和面板的样子，挖格子的手感；GPU 时间在 Metal 以外的后端上的数值。

## 修过的问题

- 碰撞体按正方形的块切，平地上每 32 列多一条竖的接缝：改成整张宽的横条，只剩横的接缝。
- 动画瓦片测试原来数格子精灵的帧，换成 `TileLayer::drawn_frame` 按场景时钟算出来的帧。
- 斜坡跨过两条之间的接缝时被切成两段，接缝下面还多一块实心矩形，物体顺着坡滑到接缝会被弹起来：坡跨过的相邻条合成一组一起建（见「场景里的地图」）。
- 只换了装饰帧（不影响导航、地形）也把 `NavGrid`、`GroundMap` 标成改过，寻路图跟着重建：只在那一格真的变了时才改。
- 改一格时整条碰撞体删掉重建，压在上面的物体收到多余的 `collide` stopped / started，正在往下穿的单向板也被换了句柄，物体被顶回板上：没变的碰撞体留着原来的实体。
- 后处理每个效果都先占一对查询，着色器没编好跳过的那一遍也解析：查询在开 pass 时才分。
- 一帧超过 16 个 pass 时后面的悄悄不量：上限提到 64，超了在面板上写「GPU ≥ x ms」，`perf` 事件带 `gpuTruncated`。
- Rust 里打开的性能面板，第一帧被没写 `perf` 的场景关掉：只在声明真的变了时才写开关。
- 走地面的寻路图只按地图和角色记，地图整张重建、挪了、变大小时还先用旧图：键里加上地图的左上角和行列数，整张重建时当场建。
- 粒子裁剪、性能曲线用的是上一帧的相机（排在相机跟随前面）：排到 `camera_rig`、`match_2d` 后面。
- GPU 读回同时好了几帧时可能用旧的那一帧：取帧号最新的。
- 点地图（`tap`）另存一份格子数据：直接读 `TileLayer`。
- `setTiles` 改一张本身无效的地图时说「没有这个 id」：分开提示。
- 瓦片和精灵同 z 时谁在上面看贴图号：瓦片固定在下面。
