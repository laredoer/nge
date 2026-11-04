# M1：2D 核心

> 前置：M0 已完成（渲染线程、平台层）。

## 目标

做出能写 2D 游戏的最小引擎：ECS、精灵批量绘制、纹理和图集、2D 相机、统一的 Pointer 输入，并提供 Rust API。

**验收标准**：`examples/sprites` 示例里 10,000 个移动的精灵，用 release 构建在 iPhone 16 Plus 上跑到 60fps，在 Mac 上跑到 120fps，最长帧间隔不超过 2 帧。

## 选型

| 用途 | 库 | 原因 |
|---|---|---|
| ECS | `bevy_ecs` 0.20（单独使用） | 成熟，查询和调度都完善，支持动态组件，后面做 TSX 桥接也用得上 |
| 数学 | `glam` 0.34 | 事实上的标准库，bevy_ecs 生态也用它 |
| 图片解码 | `image` 0.25（只开 png 特性） | 先只支持 PNG，体积小 |

## 新增 crate

- `nge-ecs`：`App` 封装（World、Schedule、插件），内置 `Time`、`Input`、`Outbox` 三个资源；实现 `nge_core::Game`，把引擎回调接进 ECS。
- `nge-render2d`：精灵批量渲染器、纹理和图集、`Camera2d`。

`nge-core` 不变，它不依赖 ECS。以后不想用 ECS 的游戏，可以直接实现 `Game`。

## API 形态（Rust）

```rust
use nge_ecs::prelude::*;
use nge_render2d::prelude::*;

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(Render2dPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (move_sprites, on_pointer));
    app
}

fn setup(mut commands: Commands, mut assets: ResMut<Assets>) {
    let texture = assets.load_png(include_bytes!("../assets/ships.png")).unwrap();
    let atlas = assets.atlas(texture, Grid { cols: 4, rows: 4 });
    commands.spawn(Camera2d::default());
    commands.spawn((Sprite::from_atlas(atlas, 0), Transform2d::from_xy(0.0, 0.0)));
}

fn on_pointer(input: Res<Input>, mut outbox: ResMut<Outbox>) {
    for p in input.pointers_pressed() {
        outbox.emit("tap", json!({ "x": p.world.x, "y": p.world.y }));
    }
}
```

平台层注册时只需要传 `nge_ecs::factory(app)`。

## 实现和草案的差异

- **App 用 `&mut self` 链式调用**（和 bevy 一样），这样插件的 `build(&self, app: &mut App)` 和用户代码可以共用同一套方法。
- **排序改成先按 z、再按纹理**：草案写的是按（纹理, z）排序、每张纹理只 draw 一次，但这样做时，不同纹理之间的层叠顺序会出错。现在 z 相同的精灵按纹理分组，相邻且纹理相同的合成一次 draw；只用一张图集时，整帧只有一次 draw。
- **`asset:/` 路径延后到 M5**：目前先用 `Assets::load_png(include_bytes!(..))` 和 `add_rgba`。
- **游戏 crate 要直接依赖 `bevy_ecs`**：`derive(Component/Resource)` 展开后引用的是 `bevy_ecs::` 路径。以后做门面 crate `nge` 时再统一导出。
- **新增调度和资源**：除了 Startup / Update，还有 PreUpdate / PostUpdate 两个调度，以及 `Props`、`Screen` 两个资源。

## 验收记录（2026-11-03）

- **单元测试**：nge-ecs 10 个，nge-render2d 12 个。另有 2 个离屏 GPU 测试，验证 uv 方向、y 轴朝上和 z 顺序。
- **Mac（release，窗口 900×700）**：10,000 个精灵稳定在 120fps，CPU 约 17%。最长帧间隔大多在 9–11ms，偶尔会到 13–21ms。对照组用 0 个精灵跑，同样会出现 21–39ms 的毛刺，说明毛刺来自系统 / 窗口合成，和精灵负载无关；要在窗口位于前台、没有遮挡的条件下再确认一次。
- **iPhone**：还没测。测试命令：`NGE_EXAMPLE=sprites examples/bounce/scripts/ios-device.sh`。

## 关键设计

1. **精灵批量绘制**：单一管线，用实例化绘制四边形。每个实例的数据是 `pos / size / rotation / uv_rect / color / z`，一共 48 字节。每帧先按 z、再按纹理排序，相邻同纹理的合成一次 draw（见下文「实现和草案的差异」）。实例缓冲按容量翻倍增长。
2. **坐标系**：世界坐标的单位是 point。`Camera2d` 有 position、zoom、rotation 三个属性，生成正交 view-proj 矩阵；默认原点在视口中心，y 轴朝上。
3. **输入**：NSH 的 touch command 进 `Input` 资源之后，转换成 `Pointer { id, phase, screen, world }`，其中 world 坐标经过主相机反投影。每帧末尾清掉边沿状态（pressed/released）。
4. **资源**：`asset:/` 路径从 App 包里的资源目录读取；纹理在渲染线程上同步加载，异步加载放到 M5。
5. **和 NSH 的通信**：NSH 发来的 command 变成 ECS 的 `NshCommand` 事件；游戏写入 `Outbox`，每帧末尾统一 emit 出去。

## 任务拆分

1. `nge-ecs`：App、Startup/Update 两个调度、Time、Outbox、`factory()`，加单元测试
2. `Input`：touch 转 Pointer，边沿状态，加单元测试（反投影单独测）
3. `nge-render2d`：Camera2d 矩阵（单测），实例化精灵管线，先按 z 再按纹理的合批排序（单测）
4. 纹理和图集：PNG 加载、Grid 图集、uv 计算（单测）
5. `examples/sprites`：10k 精灵压测，界面上显示 FPS 和最长帧间隔
6. 把 `examples/bounce` 迁到 ECS（可选，验证 API 好不好用）
7. 真机验收：iPhone 和 Mac 都用 release 构建

## 已知限制 / 后续

复审时提出、这次暂时没做的：

- **10 万精灵规模的 CPU 开销**：改成用 u64 键 `sort_unstable`；z 不变时跳过排序；用 `write_buffer_with` 省掉一次拷贝；加视口剔除。
- **图集和颜色**：图集格子边缘会渗色（uv 向内缩半个 texel，或者给图集留 padding）；改用预乘 alpha；加 Nearest 采样选项；颜色改成 Unorm16 存，减轻暗部色带。
- **层叠顺序**：z 相同、纹理相同的精灵，谁在上面取决于 ECS 存储顺序，despawn 之后可能对调。
- **相机**：`Pointer.world` 用的是上一帧的相机；有多个 Camera2d 时取第一个。（`SpriteRenderer` 每帧都清屏的问题已在 M4 改成由 `RenderFrame` 决定。）
- **API 缺失**：Sprite 的 flip / anchor、`world_to_screen`、sRGB 颜色辅助函数、可见性组件、父子变换、固定步长、键盘和鼠标输入。
