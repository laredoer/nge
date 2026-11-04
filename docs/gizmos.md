# 调试绘制（Gizmos）和物理调试视图

> 前置：2D 渲染（[m1-2d-core.md](m1-2d-core.md)）、2D 物理的空间查询（[physics2d.md](physics2d.md)）。

## 目标

- 系统里随手画线、框、圆看中间结果（射线、AI 视野、触发区），不用自己生成精灵实体、下一帧再删。
- 一个开关把所有碰撞体的轮廓画出来，排查「为什么撞上了 / 为什么没撞上」。

**验收标准**：`examples/physics` 的「查询」可视化和平台跳跃的 Q 调试视图改成用 `Gizmos`，不再生成实体；平台跳跃按 Q 时同时画出所有碰撞体的轮廓（地面、平台、金币、敌人、角色）。

## `Gizmos`（`nge-render2d`）

系统参数（`SystemParam`），即时模式：这一帧画的只显示这一帧，下一帧要再画。

```rust
fn show(mut gizmos: Gizmos) {
    gizmos.line(a, b, 2.0, RED);
    gizmos.rect(center, size, BLUE.with_w(0.2));            // 实心
    gizmos.rect_outline(center, size, rotation, 2.0, GREEN); // 空心，可以转
    gizmos.circle(center, radius, 2.0, YELLOW);             // 空心
}
```

- 坐标是世界坐标，宽度、大小是世界单位（point），跟着相机缩放。颜色是线性 RGBA（`Vec4`），和 `Sprite::color` 一样。
- 画在所有东西上面：z 固定是 `GIZMO_Z`（1000），同一帧里后画的盖住先画的。
- 实现：往已有的 `SpriteInstances` 里放白色纹理的实例（一条线一个转过的矩形，圆按 32 段折线），和精灵、粒子一起合批；`SpriteInstances` 每帧在 `PreUpdate` 里清空，所以在 `Update` / `PostUpdate` 里画都行，别在 `PreUpdate` 里画。
- 坏的输入（NaN、长度 0 的线、半径不是正数）画不出来，直接跳过。空心矩形一边是 0 时画成一条线，实心矩形不画。
- 半透明颜色的空心矩形、圆，接缝处两条线重叠，会亮一点。

## 物理调试视图（`nge-physics2d`）

- 资源 `PhysicsDebug { enabled: bool }`，默认关。开着时 `PostUpdate` 里（物理一步之后）把每个碰撞体的轮廓画出来：矩形按它的角度转，圆画圆加一条半径（看得出转了多少）；有 `DrawAt` 的画在插值过的位置上，和精灵贴着，地图这种只有碰撞体的用物理里的位置。没有 2D 渲染（没有 `SpriteInstances`）时什么也不画。
- 颜色按种类：固定刚体和只有碰撞体的（瓦片地图）灰色、动态刚体绿色、kinematic 蓝色、sensor 黄色（半透明）。
- 动态、kinematic 刚体的轮廓画在插值位置上，比碰撞体的真实位置最多落后一步；查快速运动的刚体为什么撞上时记着这一点。
- 按实体排序、sensor 最后画，增删实体时重叠处谁盖住谁不变；这一帧刚删掉的实体不画。
- 只是看，不影响模拟。

## 不做

- 3D 的调试绘制、文字标签、虚线、画在屏幕坐标上（HUD 用 NSH 写）。
- TSX 开关物理调试视图：需要时加一个 `scene.physics.debug`。

## 验收记录

- `cargo test --workspace` 通过：`Gizmos` 的线、矩形、空心矩形（含转角补缝和旋转）、圆、坏输入；`PhysicsDebug` 默认关、开着时每种碰撞体的轮廓和颜色、没有 2D 渲染时不画；两个示例的调试绘制测试改成数 `SpriteInstances` 里 z 是 `GIZMO_Z` 的实例。
- **修过的问题**：物理调试视图会给这一帧刚删掉的实体在物理位置多画一帧轮廓，改成跳过；轮廓按 `HashMap` 的顺序画，增删实体时重叠处颜色会变，改成排序；平台跳跃的 Q 开关存了两份状态，改成只用 `PhysicsDebug`。测试补上插值位置、只有碰撞体的位置、每种颜色、转过的矩形。
- 还没手测：平台跳跃按 Q 看轮廓是否和精灵贴合。
