# M1：2D 核心（草案）

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
use nge::prelude::*;

pub fn app() -> App {
    App::new()
        .add_plugins(Render2dPlugin)
        .add_systems(Startup, setup)
        .add_systems(Update, (move_sprites, on_pointer))
}

fn setup(mut commands: Commands, mut assets: ResMut<Assets>) {
    let atlas = assets.load_atlas("asset:/ships.png", Grid { cols: 4, rows: 4 });
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

## 关键设计

1. **精灵批量绘制**：单一管线，用实例化绘制四边形。每个实例的数据是 `pos / size / rotation / uv_rect / color / z`，一共 48 字节。每帧按（纹理, z）排序后合批，每张纹理只调用一次 draw。实例缓冲按容量翻倍增长。
2. **坐标系**：世界坐标的单位是 point。`Camera2d` 有 position、zoom、rotation 三个属性，生成正交 view-proj 矩阵；默认原点在视口中心，y 轴朝上。
3. **输入**：NSH 的 touch command 进 `Input` 资源之后，转换成 `Pointer { id, phase, screen, world }`，其中 world 坐标经过主相机反投影。每帧末尾清掉边沿状态（pressed/released）。
4. **资源**：`asset:/` 路径从 App 包里的资源目录读取；纹理在渲染线程上同步加载，异步加载放到 M5。
5. **和 NSH 的通信**：NSH 发来的 command 变成 ECS 的 `NshCommand` 事件；游戏写入 `Outbox`，每帧末尾统一 emit 出去。

## 任务拆分

1. `nge-ecs`：App、Startup/Update 两个调度、Time、Outbox、`factory()`，加单元测试
2. `Input`：touch 转 Pointer，边沿状态，加单元测试（反投影单独测）
3. `nge-render2d`：Camera2d 矩阵（单测），实例化精灵管线，合批排序（单测）
4. 纹理和图集：PNG 加载、Grid 图集、uv 计算（单测）
5. `examples/sprites`：10k 精灵压测，界面上显示 FPS 和最长帧间隔
6. 把 `examples/bounce` 迁到 ECS（可选，验证 API 好不好用）
7. 真机验收：iPhone 和 Mac 都用 release 构建
