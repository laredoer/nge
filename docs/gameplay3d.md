# 3D 玩法基础：角色控制器、关节、网格碰撞体、空间查询、调试画线

> 前置：3D 物理（[physics3d.md](physics3d.md)）、角色控制器（[character.md](character.md)）、关节（[joints.md](joints.md)）、调试绘制（[gizmos.md](gizmos.md)）、动画状态机（[anim-graph.md](anim-graph.md)）。

## 目标

3D 物理现在只有刚体、碰撞和射线，做不了 3D 动作游戏：

- 角色只能是动态刚体，游戏自己改速度：走不了坡和台阶，下坡飞出去，站在移动平台上不跟着走，撞墙被弹开。
- 没有关节：门的合页、链子、吊桥做不了。
- 碰撞体只有盒子、球、胶囊：形状不规则的模型（石头、地形、台阶）只能拿盒子凑。
- 查询只有射线：粗一点的东西往前扫（会不会撞上）、一片范围里有什么、离哪儿最近都查不了，也不能按分组过滤。
- 碰撞体看不见：3D 没有调试画线，碰撞体和模型对不对得上、射线打到哪儿都只能猜。

这一项把这些补上，2D 那边已有的都照着做一份 3D 的。

**验收标准**：

- `Character3d`：游戏只给水平速度和起跳速度，引擎负责重力、走坡（超过上限的爬不上、滑下来）、上台阶、贴地、沿墙滑、被脚下的平台带着走；每帧输出是否着地、地面法线、实际速度、这一帧碰到的东西（`CharacterState3d`），游戏拿去设动画状态机的参数。
- `Joint3d`：焊死、球窝、铰链（限位、马达）、滑轨（限位、马达）、绳子、弹簧；TSX 的 `scene.joints3d` 按模型 id 连，写法和 2D 的 `scene.joints` 一样。
- 网格碰撞体：从模型的网格生成一个凸包、每个图元一个凸包，或者三角网格（只给固定的）；按模型的缩放；顶点太多时报警。模型节点写 `body.shape: "convex" | "hulls" | "trimesh"`。
- 空间查询：射线之外加形状投射（球、胶囊、盒子）、重叠（形状、点）、最近点；都能按碰撞分组过滤。Rust 直接调，TSX 用 `query3d` command 问、`query3d` 事件回。
- `Gizmos3d`：3D 的即时画线（线段、盒子、球、胶囊、圆、坐标轴），默认按深度被模型挡住，也可以画在最前面；`Physics3dDebug` 开着时画出所有碰撞体的轮廓、接触点、关节、这一帧打过的射线和形状投射。
- 平台跳跃按 G 进「3D 试验场」：能走、能跳、上台阶和斜坡的 3D 角色，一扇铰链门，一串链子，一块凸包碰撞体的石头，一块来回移动的平台；Q 开关调试画线。

## 做法

### 碰撞分组和查询过滤（`nge-physics3d`）

- `Collider.group`（0 … 31）：和 2D 一样，同一组的互相不碰，0 不分组。
  - rapier 的分组：组 g 属于第 g 位、不和第 g 位碰；不分组的属于第 0 位、和谁都碰。不分组的放在第 0 位（2D 是全部位），碰撞的结果和 2D 一样，好处是查询能按位挑出「不分组的」。
- `QueryFilter` 加两项：
  - `group(g)`：像组 g 的碰撞体一样查，跳过同一组的（查自己这一队以外的东西）。
  - `only(mask)`：只查这些组的，第 g 位是组 g，第 0 位是不分组的。
  - 和原来的 `exclude(entity)`、`with_sensors()` 一起用。角色控制器按自己碰撞体的分组查。

### 空间查询

都查上一步结束时的世界；坏输入（不是有限数、方向是 0、距离或大小不是正数）返回空。

```rust
let hit = physics.cast_shape(QueryShape::Ball(0.3), from, Quat::IDENTITY, forward, 2.0, QueryFilter::default().exclude(me));
let near = physics.overlap(QueryShape::Box(Vec3::splat(4.0)), center, Quat::IDENTITY, QueryFilter::default().only(1 << ENEMY));
let p = physics.closest_point(point, 10.0, QueryFilter::default());
```

| 方法 | 结果 |
|---|---|
| `raycast(origin, dir, max, filter)` | 原来就有：第一个碰到的 `RayHit { entity, point, normal, distance }` |
| `cast_shape(shape, origin, rotation, dir, max, filter)` | 形状沿 `dir` 扫过去第一个碰到的 `ShapeHit { entity, point, normal, distance }`：`point` 在对方表面上，`normal` 是对方表面朝外的法线，`distance` 是形状走了多远。一开始就重叠时 `distance` 是 0、法线是 `-dir`（和射线一样） |
| `overlap(shape, at, rotation, filter)` | 和这个形状重叠的实体 |
| `overlap_point(point, filter)` | 包含这个点的实体 |
| `closest_point(point, max, filter)` | 最近的碰撞体表面上的点 `PointHit { entity, point, distance, inside }`；点在实心碰撞体里面时 `inside` 为真、`point` 是点自己、`distance` 是 0 |

- `QueryShape`：`Ball(r)`、`Capsule { half_height, radius }`（竖着）、`Box(size)`（整个长宽高）。
- 一个刚体只算一次：同一个实体挂的碰撞体只有一个（`Collider` 是单个组件），多个凸包是一个组合形状。

### 角色控制器 `Character3d`

照 2D 的 `Character`（[character.md](character.md)）搬过来，底下是 rapier3d 的 `KinematicCharacterController`，上方是 +y。

- `Character3d { max_slope, step, snap, gravity_scale }`，默认 50° / 0（不上台阶）/ 0.05 / 1。长度按世界单位（默认一米一个），场景里按 point 换。要和 `RigidBody::Kinematic`、`Collider`（一般是胶囊）、`Velocity` 一起用。
- 每个物理步（和 2D 一样）：
  1. 速度加重力乘 `gravity_scale`（站在地上、没往上走时不加）。
  2. 先自己走速度 × 步长：沿墙、沿坡滑，比 `max_slope` 陡的坡爬不上，`step` 以内的台阶迈上去，`snap` 以内往下贴地。
  3. 再从走到的地方跟着脚下的东西走它这一步的位移（kinematic 平台按游戏设的下一个位置，动态刚体按速度），查碰撞时不算脚下的东西。
  4. 改速度：落地后往下的速度清零，顶到天花板往上的清零，撞墙（比 `max_slope` 陡）时朝墙的水平分量去掉（沿墙的留着）。
  5. 碰到动态刚体按角色的质量给冲量。
- 走完这一帧的步，用角色的形状往外扩一点（`0.02 × length_unit`）查贴着的碰撞体当接触；`Physics3d::contacts` 对角色返回它；脚下的东西（法线朝上、坡度不超过 `max_slope`、是动态或 kinematic 刚体）下一帧带着走。
- **输出** `CharacterState3d`（`Character3d` 的必带组件，自动插上）：
  - `grounded`、`ground_normal`（没着地是 0）、`velocity`（这一帧实际的位移 ÷ 时间，含被平台带着走的、被墙挡掉的）、`hits`（这一帧碰到的：实体和法线）。
  - 每帧走完物理写一次。游戏拿它设动画状态机：

    ```rust
    fn hero_anim(mut q: Query<(&CharacterState3d, &mut AnimGraph)>) {
        for (s, mut g) in &mut q {
            g.set_float("speed", s.velocity.with_y(0.0).length());
            g.set_float("vy", s.velocity.y);
            g.set_bool("grounded", s.grounded);
        }
    }
    ```

- 瞬移（游戏直接改 `Transform3d`）不从旧位置滑过去，清掉脚下的东西；速度是 NaN 时清零；参数不是有限数时用默认值。
- 碰到 sensor 也发碰撞事件：角色的碰撞体打开全部碰撞类型（rapier 默认不算 kinematic 和固定之间的）。

### 关节 `Joint3d`

```rust
commands.spawn(Joint3d {
    a: Some(frame), b: door,
    kind: Joint3dKind::Revolute { axis: Vec3::Y, limits: Some([0.0, FRAC_PI_2]), motor: None },
    anchor_a: Vec3::new(0.5, 0.0, 0.0), anchor_b: Vec3::new(-0.5, 0.0, 0.0),
    rotation: Quat::IDENTITY, collide: false,
});
```

- 和 2D 的 `Joint` 一样：挂在单独的实体上，`a` 是 `None` 时连世界；锚点在各自刚体的坐标里；两头还不是刚体时先不连；声明变了两头没换时原地换参数，换了两头或刚体重建了就重连；参数不对 warn 一次不连。
- `rotation` 是 b 相对 a 的朝向（2D 的 `angle`）：焊死和滑轨保持它，铰链的 `limits` 从它算起。
- 种类：
  - `Fixed`：焊死。
  - `Ball`：球窝，绕锚点随便转。
  - `Revolute { axis, limits, motor }`：铰链，绕 `axis`（a 的坐标里，会归一化）转，`limits` 是弧度，马达速度是弧度 / 秒。
  - `Prismatic { axis, limits, motor }`：滑轨，沿 `axis` 滑、不转。
  - `Rope { length }`、`Spring { length, stiffness, damping }`：只管距离；弹簧按加速度算，和质量无关。
- 做法：锚点坐标系的 x 轴转到 `axis`，b 那边再乘上 `rotation` 的逆（两个坐标系朝向一致），rapier 的铰链转 x、滑轨沿 x。马达跟速度的增益和 2D 一样是 1000。
- 范围超过一整圈、轴是 0、两头同一个刚体、绳长不是正数、弹簧参数是负数都算坏参数。
- `Physics3d::joints()`、`Physics3d::is_linked(joint)`。

### 网格碰撞体

- `Shape` 多三种（`Collider` 因此不再是 `Copy`，`Clone` 很便宜，数据在 `Arc` 里）：
  - `Convex(Arc<Vec<Vec3>>)`：一个凸包，存的是凸包的顶点。
  - `Hulls(Arc<Vec<Vec<Vec3>>>)`：几个凸包合成一个形状（rapier 的 compound）。
  - `TriMesh(Arc<(Vec<Vec3>, Vec<[u32; 3]>)>)`：三角网格，只能用在固定的（固定刚体或只有碰撞体）。挂在动态、kinematic 刚体上时 warn，换成同一批顶点的凸包。
- `MeshCollider { kind, scale, collider }` 组件：等实体有了 `Model3d`（glTF 加载完）再从模型的网格生成，插上 `Collider`；`collider` 给材质参数（弹性、摩擦、密度、sensor、事件、分组），形状换掉。
  - 顶点：每个部件（glTF 的一个图元）的网格顶点乘部件的 `local`、再乘 `scale`（可以三个轴不一样）。蒙皮的按绑定姿势。
  - `Convex`：所有顶点一个凸包；`Hulls`：每个部件一个；`TriMesh`：所有部件的三角形拼起来。
  - 生成函数是纯函数 `mesh_shape(&Assets3d, ModelId, kind, scale) -> Result<Shape, String>`，单测。重复的顶点先合并（glTF 每个面各有一份顶点）。
  - 凸包超过 `MAX_HULL_POINTS`（255）个顶点、三角网格超过 `MAX_TRIANGLES`（65536）个三角形时 warn（每个实体一次），照样建。凸包本身只留外壳上的点，几千个顶点的模型通常也就几十上百个。
  - 生不出来（没有网格、所有点共面）时 warn，不插 `Collider`。
- 有 `MeshCollider`、还没有 `Collider` 的实体，刚体先不建：不然动态刚体没有碰撞体会先掉下去。
- 模型、`MeshCollider` 不变时不重算；只改了材质参数也重算（便宜，不常改）。

### 调试画线 `Gizmos3d`（`nge-render3d`）

```rust
fn show(mut gizmos: ResMut<Gizmos3d>) {
    gizmos.line(a, b, RED);
    gizmos.cuboid(center, rotation, size, GREEN);
    gizmos.sphere(center, radius, YELLOW);
    gizmos.capsule(center, rotation, half_height, radius, BLUE);
    gizmos.axes(center, rotation, 1.0);
    gizmos.on_top().line(a, b, WHITE); // 不被模型挡住
}
```

- 资源，即时模式：每帧 `PreUpdate` 清空，`Update` / `PostUpdate` 里画。颜色是线性 RGBA。
- 盒子 12 条边；球是三个互相垂直的圆；胶囊是上下两个圆、四条竖线、两个方向的半圆；圆 32 段；坐标轴是红绿蓝三条线（x、y、z）。坏输入（不是有限数、大小是负数）跳过。
- 渲染：网格渲染器里加两条线的管线（`LineList`，顶点是位置加颜色），在网格和方片之后、同一个 pass 里画：
  - 默认的：深度比较 `Less`、不写深度，被模型挡住，自己不挡别的；
  - `on_top()` 的：深度比较 `Always`，画在所有模型上面。
  - 3D 管线是第一帧一次建好的，多两条不影响预热（见 [pipeline-warmup.md](pipeline-warmup.md)）。
- 线是 1 像素宽（wgpu 的线没有宽度），半透明照常混合。

### 物理调试视图 `Physics3dDebug`

- 资源 `Physics3dDebug { enabled, on_top }`，默认关。开着时 `PostUpdate` 里（物理之后）画进 `Gizmos3d`（没有就什么也不画）：
  - 碰撞体的轮廓，颜色和 2D 一样：固定和只有碰撞体的灰、动态绿、kinematic 蓝、sensor 黄（半透明）。球、盒子、胶囊画对应的形状，凸包画外壳的边，三角网格画每个三角形的边，组合形状画每一块。角色额外画一个坐标轴。
  - 接触点：每个接触点一个小十字和一条法线（橙色），角色的接触是它的 `hits`。
  - 连上了的关节：两个锚点之间一条粉线。
  - 这一帧打过的射线和形状投射：没碰到是暗红的线，碰到了是红线到碰撞点、绿的法线；形状投射在终点画一个球 / 盒子 / 胶囊。`Physics3d` 只在调试开着时记（`Mutex` 里的一个列表，画完清掉），关着时查询不多花时间。
- `on_top` 为真时全部画在最前面。
- 画的是物理里的位置（3D 没有画面插值，和模型一致）。

### 场景（`nge-scene`）

- 模型节点的 `body`：
  - `kind: "character"`：kinematic 刚体加 `Character3d`，参数 `slope`（度，默认 50）、`step`（默认 0）、`snap`（默认 4，point）。形状一般写 `capsule`。
  - `shape: "convex" | "hulls" | "trimesh"`：从模型网格生成，按节点的 `scale` 缩放；`trimesh` 只能配 `kind: "fixed"`。
  - `group`：碰撞分组。
- `scene.joints3d`：按模型 id 连，字段和 `scene.joints` 一样，坐标是三维的：

  ```ts
  joints3d: [
    { id: "hinge", a: "frame", b: "door", kind: "revolute", anchorA: [0, 0, 0], axis: [0, 1, 0], limits: [-100, 100], motor: { speed: 0, maxForce: 2e5 } },
    { id: "link-0", b: "ball-0", kind: "ball", anchorA: [0, 300, 0] },
  ]
  ```

  - `kind`：`fixed`、`ball`、`revolute`、`prismatic`、`rope`、`spring`。锚点、`axis` 写世界坐标，按两头模型**声明的**位置和角度换到各自身上；单支点的（焊死、球窝、铰链、滑轨）只写一个锚点时两头都用，都不写是 b 的位置；`axis` 默认 `[1, 0, 0]`。
  - 单位：铰链的 `limits`、马达速度用度、度 / 秒；滑轨用 point。弹簧默认 `stiffness` 100、`damping` 10。
  - 找不到带 `body` 的模型时 warn 一次先不连；写错的字段、种类整份场景解析失败。
- `query3d` command / 事件：

  ```ts
  const q = queries3d(() => game);
  // <GameView onQuery3d={q.handle} />
  const hit = await q.ray([0, 100, 0], [0, -1, 0], 500, { exclude: "hero" });
  const near = await q.overlap({ ball: 80 }, [x, y, z], { only: [0, 2] });
  ```

  - command `{ id, kind: "ray" | "cast" | "overlap" | "point" | "closest", from, dir?, distance?, shape?, rotation?, exclude?, sensors?, group?, only? }`；`shape` 是 `{ ball: r }`、`{ capsule: [h, r] }`（中间高、半径）或 `{ box: [x, y, z] }`。
  - 事件 `{ id, hits: [{ id, point?, normal?, distance?, inside? }] }`：`id` 是模型的 id，不是模型的碰撞体（游戏在 Rust 里加的）是 `null`。射线、投射、最近点最多一个，重叠的按实体排好。
  - 查的是收到 command 那一帧物理之后的世界。TSX 这边 `queries3d()`（`ts/physics3d.ts`）按 `id` 配对成 Promise，引擎一直不回时两秒后按空结果结束。
- 模型的 `collide` 事件、`push` 照旧。

### TSX

`ModelBody` 的 `kind: "character"`、`slope` / `step` / `snap`、新的 `shape`、`group`；`Joint3dNode`、`Scene.joints3d`；`queries3d()`、`<GameView onQuery3d>`。

### 演示：平台跳跃的「3D 试验场」

平台跳跃是横版 2D，3D 角色在关卡平面里走不开；把演示放在一个单独的小场景里，按 G 进出（桌面上），不碰原来的关卡：

- `lab.ts` 的 `labScene()` 是一份完整的场景（`level: "lab3d"`，透视相机，没有 2D 的角色和关卡），模型和关节的布置放在 `lab.json` 里，Rust 测试也读它；`game.tsx` 按 G 切换 `scene` 用哪一份。在试验场里存档、语言、联网、选关这些按键不响应。
- 场景（point 为单位，地面 y = 0）：
  - 一块 1600 见方的地面；一段 20° 的斜坡、一段 40° 的陡坡（爬不上）——斜坡是三角网格；四级台阶（每级高 16，角色的 `step` 是 20），一个模型四个图元，`hulls` 每级一个凸包；
  - 一扇铰链门（绕竖轴转，限 ±100°，带一点阻力的马达），角色走过去能推开；
  - 一串五节的链子（球窝）吊着一个重球，角色撞上去会荡；
  - 一块用 `convex` 的石头（不规则的二十面体），能被推着滚；
  - 一块来回移动的平台（kinematic），站上去跟着走。
- Rust（`examples/platformer/src/lab.rs`）：
  - WASD / 方向键走（相机看向 -z，W 往里），空格跳（着地才能跳）；掉出去回到起点。
  - 相机跟着角色、在后上方；平台按时间来回挪。
  - Q 同时开关 2D 和 3D 的调试视图；试验场里调试开着时，角色每帧往下打一条射线、往前扫一个球，画出来。
- 资源：`tools/gen_glb.py` 生成 `lab-*.glb`（地面、斜坡、台阶、门、球、石头、角色、平台），只用标准库。

## 不做

- 角色之间互相推、角色当别的角色的地面（和 2D 一样）；横着撞过来的 kinematic 物体不把角色推开。
- 球窝的锥形限位、扭转限位；关节断裂和受力读数；多体关节。
- 凸分解（V-HACD）：`hulls` 只按图元分，凹的单个图元还是一个凸包；三角网格给动态刚体用。
- 凸包顶点太多时自动简化：只报警（凸包自己就只留外壳上的点）。
- 动画的蒙皮跟着碰撞体变：碰撞体按绑定姿势，一次生成。
- 线宽、虚线、文字标签、屏幕坐标的 3D 画线。
- TSX 开关 3D 物理调试视图（Rust 设 `Physics3dDebug`）；查询结果的 `query3d` 批量合并。
- 3D 画面插值（照旧）。
- 试验场的触屏操作（只在桌面上按 G 进）。

## 验收

- `nge-render3d`：`Gizmos3d` 各种形状生成的顶点（个数、位置、颜色、`on_top` 分开、坏输入跳过、每帧清空）；离屏渲染：线画出来、被前面的模型挡住、`on_top` 的不被挡。
- `nge-physics3d`：
  - 查询：形状投射（球、胶囊、盒子；起点就重叠；过滤自己、sensor）、重叠、点、最近点（外面、里面）、分组（`group` 跳过同组、`only` 只挑这些组、不分组的在第 0 位），同组不碰撞。
  - 角色：落地停住、地面法线；走上缓坡、陡坡上不去也站不住；上矮台阶、高台阶上不去；沿墙滑（斜着撞墙，沿墙的分量留着）；顶天花板；被横移、上升的平台带着走；推得动箱子；瞬移；碰到 sensor 有事件；`CharacterState3d` 的速度、着地、碰撞列表。
  - 关节：铰链限位停在范围边上、马达转起来、`rotation` 改变限位的起点；球窝保持距离；焊死一起走；滑轨沿轴、限位；绳子、弹簧；坏参数不连、改对了连上；组件去掉就拆、刚体重建重连、只改马达原地换。
  - 网格碰撞体：立方体（24 个顶点）的凸包是 8 个点、包含所有顶点；`hulls` 每个部件一个；缩放；三角网格的三角形数；动态刚体上的三角网格换成凸包；共面的点生不出来；模型加载前不建刚体，加载后凸包的石头落在地面上。
  - 调试视图：每种形状画出线、颜色按种类、接触点、关节、射线记录只在开着时记，`on_top`。
- `nge-scene`：`kind: "character"` 建出角色、参数换算；`shape: "convex"` 等到模型加载完建出凸包；`joints3d` 的锚点、轴、角度换算、单位、找不到时不连；`query3d` 回的 id 和结果。
- 平台跳跃：TSX 测试检查试验场的场景（角色、门的铰链、链子、凸包石头、台阶的 `hulls`、斜坡的 `trimesh`、平台）；Rust 测试：试验场里角色能走、跳、上台阶、推开门，Q 开关 3D 调试。
- 变异检查（改坏一处、确认有测试挂、改回来）：

  | 改坏的地方 | 挂的测试 |
  | --- | --- |
  | `Gizmos3d::on_top` 画进被挡住的那组 | `gizmos::a_line_is_two_vertices_with_its_color` |
  | 画在最前的管线也用深度比较 `Less` | 离屏 `debug_lines_are_hidden_behind_models_unless_on_top` |
  | 形状投射不先查起点就重叠的 | `a_cast_that_starts_inside_hits_at_once` |
  | 查询的 `group` 不当成那一组（属于全部位） | `queries_filter_by_collision_group` |
  | 角色不自动上台阶 | `steps_up_low_ledges_only` |
  | 角色不跟着脚下的平台走 | `rides_moving_platforms` |
  | 铰链不设限位 | `hinge_swings_and_stops_at_its_limits`、`hinge_limits_start_from_the_declared_rotation` |
  | 凸包留下所有顶点（不取外壳） | `a_convex_hull_keeps_only_the_outer_corners` 等 2 个 |
  | 有 `MeshCollider` 没 `Collider` 的刚体照样先建 | `mesh_colliders_wait_for_the_model_and_then_land` |
  | 调试开着也不记查询 | `rays_and_casts_of_this_frame_are_drawn_once` |
  | 场景的关节轴不换到 a 的坐标里 | 场景 `anchors_and_axes_are_converted_from_declared_poses` |
  | `query3d` 不管 `exclude` | 场景 `rays_and_casts_hit_models_by_id` |
  | 场景的 `shape: "convex"` 不生成网格碰撞体 | 场景 `mesh_shapes_wait_for_the_model_then_use_its_hull` |
  | 场景的 `step` 不传给角色 | 场景 `character_bodies_get_a_controller_and_groups` |
  | 平台跳跃按 Q 不开关 3D 调试 | `q_toggles_the_3d_debug_view_and_the_platform_moves` |
  | TSX 的 `queries3d` 交回结果后不删等着的 | `query3d.test.tsx`「结果按 id 交回」 |

  第一版查询的过滤掩码还减掉了自己那一组，改坏它没有测试挂：碰撞体那边的掩码已经挡掉同组的，这一步多余，删了。
- 做的时候修过的问题：
  - parry 从里面开始的形状投射时报时不报（看往哪个方向扫）：先单独查起点的重叠。
  - parry 对共面的点也给一个扁的凸包：按体积判断，几乎是 0 的当成生不出来。
  - `Collider` 不再是 `Copy`（凸包、三角网格带着 `Arc`），建刚体时按 clone 比较。
- 还没手测：试验场的手感、相机、画线的样子，`game.tsx` 里按 G 进出（无头的 `nsh test` 没有游戏视图，只靠类型检查和 Rust 那边的试验场测试；这台电脑上不开窗口）。
