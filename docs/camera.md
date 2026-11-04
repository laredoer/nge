# 2D 相机：跟随、边界和震屏

> 前置：M3（TSX 声明式场景）、动画（[animation.md](animation.md)）、键盘（[keyboard.md](keyboard.md)）。

## 目标

TSX 不写每帧逻辑，也能让相机跟着一个精灵走、不拍到地图外面、在撞击时抖一下。

**验收标准**：`examples/scene` 的相机平滑地跟着飞船（点飞船、方向键移动时画面跟着走），走到边上停住不拍出边界；点飞船时屏幕抖一下。

## TSX

### `scene.camera`

```ts
camera: { x = 0, y = 0, zoom = 1, follow?: "精灵 id", smooth = 0, bounds?: { left, right, bottom, top } }
```

- 不填 `follow` 时和以前一样，相机中心在 `(x, y)`。
- 填了 `follow`：相机中心是那个精灵的位置加上 `(x, y)`（`x` / `y` 变成偏移，比如 `y: 100` 让主角在画面偏下的位置）。每帧在物理和补间之后读精灵的位置，所以跟着刚体、补间、Rust 系统改的位置都行（Rust 系统要在 `Update` 里改，或者排在 `PhysicsStepped` 之前，不然会晚一帧）。
- `smooth`：跟随时的平滑时间（秒）。0 是每帧对准；0.2 大约是 0.2 秒追上 63%，越大越「拖」。只在跟随时起作用，刚开始跟随、换了跟随目标、或者目标刚出现的那一帧直接对准，不从别处滑过来。
- `follow` 的精灵不存在时相机停在原地（照样按 `bounds` 限制），不回到 `(x, y)`；精灵出现后直接对准。
- `bounds`：世界坐标里相机能拍到的范围。视野（视口大小 ÷ `zoom`）的边不超出它；视野比范围还大的那一轴就对准范围的中心。跟随和不跟随都起作用，视口大小变了会重新算。按相机实际的 `zoom` 算（Rust 改了 `Camera2d.zoom` 也对）。

### `shake` command

```ts
shake(game, { strength = 8, duration = 0.3 })
```

- 相机在 `duration` 秒里抖动，幅度 `strength`（屏幕上的 point，不随 `zoom` 变）线性衰减到 0。
- 抖动叠加在相机当前位置上（包括 Rust 系统自己设的位置），不受 `bounds` 限制，结束后回到原位。比一帧还短的也会抖一帧。抖动是确定的（按时间的正弦组合），不用随机数。
- 抖着的时候再来一个：留下当前幅度更大的那个。
- 抖动期间点击的世界坐标也跟着抖动后的相机算（最多差 `strength` 个 point）。

## 实现

- `sync_camera`（PreUpdate，`scene` 变了时）：存下相机节点；不跟随时马上把相机放到 `(x, y)`（按 `bounds` 限制），这样同一帧的点击用的是新相机。
- `camera_rig`（PostUpdate，物理之后）：跟随时算目标位置，按 `smooth` 指数逼近；按 `bounds` 限制；最后加上震屏偏移写进 `Camera2d`，并记下这次的偏移。下一帧先从相机位置里减掉上次的偏移，得到不含震屏的位置再算，所以 Rust 游戏自己改的 `Camera2d.position` 抖完、视口变了都还在。不跟随、不抖、视口没变的帧不写，Rust 游戏自己改 `Camera2d` 的话不会被每帧覆盖（`scene` 里的 `camera` 变了时还是以声明为准）。
- 平滑是 `1 - exp(-dt / smooth)`，和帧率无关。

## 不做

- 相机旋转、死区（主角在中间一小块里移动时相机不动）、多目标取中点、`zoom` 的补间：需要时再加。
- 3D 相机（`camera3d`）不变。

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-scene` 的 `camera` 模块新增 10 个（跟随加偏移、Rust 改的位置也跟；平滑和开始时直接对准；目标不存在时停住、出现后对准；边界、zoom、视口变化；目标没出现时边界照样生效、换目标直接对准、目标没了停住；震屏衰减回原位、按 zoom 缩小、弱的不接管、强的接管、比一帧短的也抖一帧；Rust 驱动的相机抖完和视口变化后还在原位、每帧绝对赋值时不多减偏移、不跟随不抖时不被覆盖），原来 `sync.rs` 里的 2 个相机测试挪进来；`examples/scene` 的测试加上相机跟随和 `shake`。
- **修过的问题**：修了 Rust 驱动的相机被震屏 / 视口变化拉回声明位置、跟随目标没出现时边界不生效、换目标不对准、按声明而不是实际 `zoom` 算、`shake` 收 Infinity、比一帧短的不抖、绝对赋值时多减一次偏移。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过。
- **实跑**：`scene`（`nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接。手测：跟随、边上停住和震屏效果可以。
