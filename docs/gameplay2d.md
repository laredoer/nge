# 2D 玩法补齐：单向平台、瓦片斜坡、动画瓦片、走地面的寻路、行为树

> 前置：角色控制器（[character.md](character.md)）、瓦片地图（[tilemap.md](tilemap.md)）、寻路和 AI（[ai.md](ai.md)）、移动平台（[moving-platforms.md](moving-platforms.md)）、关卡编辑器（[editor.md](editor.md)）。

## 目标

前面几项的「不做」里留下了几块 2D 玩法的缺口：

- 没有单向平台：从下面跳不上去，站上去也下不来。
- 瓦片地图只有方块：斜坡只能拿转过角度的精灵凑，陡坡的坡脚还是个尖角。
- 瓦片不会动：水面、火把只能另外摆精灵。
- 寻路只有给会飞的单位用的格子 A*：走地面的敌人不会跳上平台，也不会从边上落下去。
- AI 只有状态机：「巡逻、看见就追、追丢了回去」这种有优先级的逻辑写起来全是状态切换。

**验收标准**：

- 单向平台：从下面能跳上去、站在上面不掉、按「下」加跳穿下去；挂在刚体、碰撞体上，或者给瓦片地图的某种瓦片用；角色控制器和动态刚体都对。
- 瓦片斜坡：45° 和缓坡（两格升一格）的斜坡瓦片，上坡下坡顺着走，坡和坡、坡和平地相接处不卡；陡坡坡脚不再被当成墙。
- 动画瓦片：某种瓦片按帧序列循环，只换受影响的格子，不重建地图。
- 走地面的寻路：节点是能站的地方，连线有走、跳、落三种（从单向平台穿下去算落），跳的连线按最大跳高和跳远算；敌人沿路径追到目标，会跳上平台、从边上落下。
- 行为树：顺序、选择、并行、取反、重复、冷却、等待，条件和动作，黑板；Rust 能直接用，TSX 能在精灵上声明、能改黑板、能收事件；和状态机共存。
- 小项：瓦片地图点某一格（`tap` 带格子）、每种瓦片不同的摩擦和弹性（冰面）。
- 平台跳跃有一关放上单向平台、斜坡瓦片、水面和火把，加一个行为树驱动、走地面寻路的敌人：巡逻，看见就追，会跳上平台，追丢了回巡逻线；编辑器能放这些新瓦片。

## 做法

### 单向平台（`nge-physics2d`）

`Collider` 加 `one_way: bool`。单向平台只挡「从它上面来的」，「上面」按碰撞体自己的 +y（转过的平台按转过的方向）。

- **角色控制器**：控制器每一步查碰撞时用一个过滤：单向平台只在角色整个在它顶面以上（按平台自己的坐标比包围盒，留 1 point 的余量，控制器和地面之间本来就有间隙）、又没在往下穿时才算。往上跳时角色还在平台下面，查询看不见它，直接穿过去；落下来时已经整个在上面了，照常站住。被平台带着走的那次查询也用同一个过滤。
- **动态刚体**：单向平台的碰撞体开 rapier 的 `MODIFY_SOLVER_CONTACTS` 钩子，`step` 时传一个 `PhysicsHooks`：动态刚体和单向平台的接触，法线（换成从平台指向对方）和平台的上方差 60° 以内的留着，别的清掉。一对一旦从下面、侧面穿进去就记成「穿过中」（存在 manifold 的 `user_data` 里），穿到整个出来才重新挡，免得穿到一半被往上弹。「整个出来」按几何算：对方的包围盒底边（按平台的坐标）不低于平台顶面减余量、又没在往上走（相对平台）。刚好擦过顶面的那一帧 manifold 的法线还是旧的（朝侧面），这时把法线改成平台的上方、接触点挪到平台顶面和对方底边，不然箱子会被旧法线推下去。和 rapier 自带的 `update_as_oneway_platform` 一个思路，只是按世界坐标的法线算，转过的平台也对。
- **穿下去**：`Physics::drop_through(entity)`：找脚下贴着（`CONTACT_GAP` 以内、整个在顶面以上）的单向平台，记进「正在往下穿」的集合，这一对在包围盒外扩 `CONTACT_GAP` 也不相交之前都不挡；顺便叫醒睡着的动态刚体、把角色的着地状态清掉（这就开始加重力、不贴地、不跟着平台走）。头顶贴着平台底的不算，返回 `false`，游戏照常起跳；脚下同时踩着别的实心东西（比如站在实心块和木板的接缝上）也返回 `false`，什么都不记。贴着找到的平台下面紧挨着（`CONTACT_GAP` 以内、横向有重叠）的单向平台一起记上，叠着的几层木板一次穿过。每帧物理走完以后删掉已经分开的对（按这个角色正在穿的几块平台合起来的范围算，叠着的下面那层还没碰到时不删）；角色站到别的东西上了也删掉，不然下次落回这块平台会直接穿过去。
- **调试画线**：`Shape::Slope` 画外框；单向平台和普通碰撞体一样画。

### 瓦片斜坡

`Shape::Slope { size, left, right }`：一个格子大小的四边形，底边是整条，顶边从左边的高 `left` 到右边的高 `right`（都是 `size.y` 的几分之几，0 … 1），用 `convex_hull` 生成；数值不对（NaN、两边都是 0、大小是 0）时不生成碰撞体、warn。

瓦片地图的 `slopes: [{ frame, left, right }]` 把某种帧号当斜坡。45° 上坡是 `0 → 1`，22.5° 的缓坡两格一组（`0 → 0.5`、`0.5 → 1`）。

坡和坡相接处不卡靠「并成一块」（`nge-scene` 的 `tile_slopes.rs`）：

- 从左往右找，顺着坡度一样、首尾接得上的斜坡格连成一条直线（上坡到顶就跳到右上一格，下坡到底就跳到右下一格）。
- 这条线下面一直到最低那一格的底都是实心格时，整条线和下面的实心格并成一个凸四边形碰撞体；被并进去的实心格不再参加矩形合并。坡顶和坡脚接着平地的地方，平地的矩形顶边和坡的端点正好对上，没有凸起的棱角。
- 下面空着（悬空的坡）时每格单独一块，不把空的地方填上。

「陡坡坡脚是尖角」：[character.md](character.md) 记过 40 多度的转过角度的矩形坡脚贴着地面时会被当成墙。这次按平台跳跃那样的角色大小和速度把 40° … 49° 扫了一遍（地面在坡脚处断开，像瓦片地图合并出来的两块），现在都走得上去，没再复现；留了一个回归测试。瓦片斜坡都是并好的整块，不会出现这种尖角。

### 动画瓦片、材质、点格子（`nge-scene`）

- `animated: [{ frame, frames, fps }]`：帧号是 `frame` 的格子每个挂一个循环的帧动画（和精灵的 `anim` 用同一个 `FrameAnimation`），同时生成的一起换帧。只换这些格子的帧，不重建地图、不动碰撞体。动画瓦片的帧号不在 `solid` 里就只是画面（水、火把不挡人）。
- `materials: [{ frame, friction?, restitution? }]`：没写的用地图的。合并碰撞体时按材质分开合并（实心格、单向平台都是），冰面和普通地面不并成一块。
- `tap: true`：点到地图时发 `tap`，带 `cell: [列, 行]` 和那一格的 `frame`。空格不算点中（往下接着找）。点中的顺序：z ≥ 0 的精灵、模型，然后开了 `tap` 的瓦片地图和 z < 0 的精灵按 z 排（z 大的先，一样大时精灵先）。带视差的地图按画出来的位置算。
- 一个帧号写在几处时：斜坡优先，然后单向平台，最后实心；斜坡帧又写进 `oneWay` 时 warn 一次。
- 换规则（比如改了 `animated`）会重建地图，动画按场景时钟接着播，不从第一帧重来。
- `slopes` / `animated` / `materials` 逐项解析，写错的那一项 warn 后丢掉，别的照常（和 [m3-tsx.md](m3-tsx.md) 的规矩一样）。
- 会飞的单位的 `NavGrid`：斜坡挡住，单向平台不挡（飞得过去）。
- 精灵刚体的 `body.oneWay`：精灵刚体（比如移动平台）也能当单向平台。

### 走地面的寻路（`nge-ai` 的 `ground.rs`）

地形 `Terrain`：每格 `Empty` / `Solid` / `OneWay` / `Slope { left, right }`。坐标按格算，y 往上，地图顶边是 0。

- **节点**是能站的地方：实心或单向平台上面的空格，或者斜坡格自己（上面那格不是实心的）。记着地面左右两头的高。
- **走**：左右相邻、地面接得上（同高或者上下差一格但端点对上，比如坡顶接平地）。
- **能力** `Walker { reach, half, height, max_slope }`：跳跃能力、半宽、身高（格）、能走的最大坡度。比 `max_slope` 陡的坡不算能站的地方；场景里按精灵的大小和瓦片大小换算，`max_slope` 和角色控制器的一样。
- **落**：从边上走出去，往下第一个能站的地方，一路上没有实心的。站在单向平台上时还有一条**穿**（`Drop`），穿过下面连着的几层单向平台，落到第一个不是单向平台的能站的地方（和物理里一次穿过叠着的几层一致）。
- **跳**：跳跃能力 `Reach { height, distance }`（格）由起跳速度、重力、水平速度换算（`Reach::from_physics`）。落点高出 `dy` 时，能跳的水平距离是 `distance × (1 + √(1 − dy / height)) / 2`（满高的抛物线在落点那一刻走过的距离）。一路按 0.25 格一份取样（4 … 48 份），按 `Walker` 的半宽、身高的方块查（整段都是空格的范围直接跳过）：实心格和斜坡顶边以下挡住；单向平台往上时穿过、往下落时接住（落点那块除外）。满高碰头时再试矮一点的跳（满高的 0.75、0.55、0.4），矮的跳飞得短、横着也跳不了那么远。连线记着用的跳高。
- **建图的快慢**：按列建索引；同一段连着能走的地面只从一头试跳（走得到的地方不用跳）；每列只试比起点高的和第一个低的落点，落点上面有遮挡的跳过。400 × 60 格、两层地面加平台的图建一次十几毫秒，有回归测试（< 20 ms）。
- **找路**：Dijkstra，走 1、落 `1 + 落差 × 0.5`、跳 `1.5 + 横向 + 竖向 × 0.5`，展开上限 4096。`find_avoiding` 可以跳过一些连线（场景里用来不再试老失败的跳）；`find_near` 是目标下面没有能站的地方（比如悬在坑上）时，找到离它最近的能站的地方的路。

### 行为树（`nge-ai` 的 `bt.rs`）

`Tree<L>`：根节点、黑板、树自己的时钟。叶子是什么由用的人定，每帧 `tree.tick(dt, &mut |leaf, blackboard| -> Status)`。

| 节点 | 语义 |
| --- | --- |
| `sequence` | 一个一个跑，有一个失败就失败、全成了才成；还在跑的下一帧接着跑那一个 |
| `selector` | 一个一个试，有一个成了就成、全失败才失败 |
| `.reactive()` | 顺序 / 选择每帧从第一个孩子重新查，条件变了能打断后面在跑的（被打断的回到没开始） |
| `parallel(n)` | 每帧跑所有没跑完的孩子，成了 `n` 个就成，剩下的凑不够了就失败 |
| `invert` | 成败对调，在跑不变 |
| `repeat(times)` | 孩子成了下一帧再跑（`None` 一直跑），成了 `times` 次才成；孩子失败就失败 |
| `cooldown(s)` | 孩子跑完（成或败）以后 `s` 秒内直接失败；被打断不清冷却 |
| `wait(s)` | 等 `s` 秒（从开始那一帧的开头算） |
| `leaf(L)` | 交给回调 |

在跑的叶子被打断（反应式前面变了、并行凑够了、整棵树 `halt`）时，`tick_halting` / `halt_with` 的回调拿到这个叶子收尾。树的时钟用 `f64`，跑几天也不丢精度。

黑板的值是 `Bool` / `Num` / `Text` / `Pos`。和 `Fsm` 不冲突：状态机管「现在是什么状态」（比如动画），行为树管「这一帧做什么」，叶子里读写状态机，有一个测试这么用。

### 场景里的行为树（`nge-scene` 的 `behavior.rs`，feature `ai`）

精灵的 `behavior: { tree, blackboard?, speed?, jump? }`，配 `body: { kind: "character" }`。

- 树用 JSON 写，`type` 是节点名：`sequence` / `selector`（`reactive`）、`parallel`（`succeed`）、`invert`、`repeat`（`times`）、`cooldown`（`seconds`）、`wait`（`seconds`），叶子 `see`（`target`、`range`，有导航网格时还要没墙挡着）、`near`、`check`（黑板上的值是真的）、`set`（`null` 删掉）、`chase`（`target`）、`goto`（`x`、`y`）、`home`（声明的位置）、`emit`（发 `behavior` 事件 `{ id, name, x, y }`）。
- `blackboard` command：`{ id, key, value }` 改黑板（`value: null` 删掉；没写 `value` 时 warn，黑板不动）。`parallel` 的 `succeed` 比孩子多（永远成不了）时 warn。
- 树变了才重建（从头跑、黑板回到初值）；TSX 每帧声明同一棵树不会重置。只改了位置只换 `home`。写错（不认识的节点、负的秒数）时 warn 一次，只丢 `behavior`，精灵照常生成（字段按 `sprites.behavior` 单独解析）。
- 走路：每个有行为树的精灵找它所在的瓦片地图（有 `GroundMap` 的，横竖都在地图范围里；几张都盖得住时用脚下有能站的地方的那张），按自己的速度、起跳速度、大小建图（打 0.85 的折扣，跳得到的一定够得着；同一张地图、同样能力的共用，地图变了重建）。`see` 在这张地图上按格子查视线（斜坡、单向平台不挡），不在地图上时用 `NavGrid`。`chase` / `goto` / `home` 站在地上时找一次路：到了（横着 0.4 格、竖着 1 格以内）是成，找不到路是失败，别的是在跑，按第一步写速度：
  - 走、落：朝下一格走（落的时候走出边，半空中朝落点挪）。
  - 跳：先走到起跳格中间，再按连线的跳高算竖直速度 `√(2gh)`、按落地时间算水平速度，半空中保持这个速度。
  - 穿：走到格子中间，`drop_through`。
  - 半空中不重新找路，接着按起跳时算好的走。
  - 目标悬在坑上（下面没有能站的地方）时走到离它最近的能站的地方停下（在跑，不是成）；没有路、站的不是能站的地方时只朝目标走，前面是坑边就停下，不往下掉。
  - 只有在跑的叶子占着目标：`chase` 失败了马上放开，后面的叶子不会接着按它的路走。
  - 卡住：跳或穿下去没落到连线的终点（落地的时候不在那一格，或者 `drop_through` 不成）算失败一次，同一条连线失败 3 次就 10 秒不用它，这次的 `chase` / `goto` 失败。
  - 掉出地图（比地图底边低 2 格）：发 `behavior` 事件 `fell`，回到出生的地方（`home`），速度清零，树打断从头跑。
- 联网时行为树在每个客户端各自算（和别的精灵的物理一样），不同步；要一致的游戏由主机算、把位置发出去。

### TSX

- `TilemapNode` 加 `oneWay`、`slopes`、`animated`、`materials`、`tap`；`Tap` 加 `cell`、`frame`；精灵刚体加 `oneWay`。
- `SpriteNode.behavior`、`BtNode`、`bt.*` 拼树的小函数、`blackboard()`、内置事件 `behavior`。
- `dropThrough(game, { id })`：让角色从脚下的单向平台穿下去，回 `dropThrough` 事件 `{ id, ok }`（`ok` 是穿没穿成）。
- `VirtualStick` 加 `down`：摇杆往下推过一半时按着这个动作（来源是 `<source>:down`），回来或抬起时松开；横着推不按。
- 关卡文件（`ts/level.ts`）的瓦片层加可选的 `oneWay`、`slopes`、`animated`、`materials`，逐项校验（帧号在图集里、坡高 0 … 1 且不都是 0、动画有帧、fps 是正数、材质不是负数），`toTilemap` 原样带上；`emptyLevel` 从图集抄。

### 演示：平台跳跃第二关「石阶」

- 瓦片图集 `tiles.png` 由 `tools/make_tiles.py` 生成（只用标准库）：前 3 格照抄原来的石块、草地、木板，后面加单向木板、45° 上下坡、22.5° 上下坡各两段、三帧水、两帧火把；图集从 `[3, 1]` 改成 `[16, 1]`，原来的帧号不变（第三关的 Tiled 图块集跟着改了宽度）。
- 「石阶」（关卡文件 `examples/editor/levels/stairs.json`）：第一组台阶换成 45° 的坡，最后一组换成 22.5° 的缓坡，两块木板换成单向木板，第一个坑里是水，地上三个火把；中间那只巡逻的敌人换成「走地怪」。
- 走地怪（`enemy-walker-*@<第几轮>`，踩它、被它撞和别的敌人一样）：比角色慢（110），跳得比角色高（720，够得着三格高的单向木板）。行为树：看见角色（8 格以内、没墙挡着）就追，一次看见只喊一声（黑板上记 `spotted`，跟丢了回去巡逻时清掉；另外 3 秒最多一次；头上飘「!」、响一声）；看不见了回出生点，在出生点和右边 3 格之间来回走，两头各停 0.6 秒。按 R 重开时 id 里的轮数加一，引擎重建走地怪，回到出生点、树从头跑。
- 角色按 ↓（手柄的十字键下、左摇杆往下，触屏摇杆往下推过一半）再按跳：站在单向木板上就穿下去。第二个角色（P2，↓ 是 V）也一样。新动作 `down` 在改键面板里也能改。
- 编辑器的瓦片栏多了这些新瓦片，刷上去存进关卡文件；编辑器的画布不要碰撞体，单向平台和斜坡也一样清掉。
- 编辑器打开旧关卡（用这张图集、`grid` 还是 `[3, 1]`、没写单向板和斜坡的规则）时按现在的图集升级：`grid` 换成 `[16, 1]`，没写的规则补上，状态栏说一声，保存后生效。瓦片栏只列这一层图集里有的帧。

## 修过的问题

- 头顶贴着单向平台底的角色按下加跳时被当成「站在上面」穿下去，结果不跳：`drop_through` 只算脚下的平台。
- 瓦片斜坡每格一块时，相邻两块和下面合并出来的实心矩形在坡面上留下棱角，走到接缝处顿一下：把一条直线上的坡连同下面的实心格并成一块。
- 走地怪追一个悬在坑上的目标时找不到路，没有路就直接朝目标走，掉进坑里：找离目标最近的能站的地方走过去停下；没有路时前面是坑边就停。掉出地图的回出生点、发 `fell`。
- `chase` 失败以后目标还留着，后面的叶子（比如巡逻的 `goto`）头一帧按追人的路走：只有在跑的叶子占着目标。
- 站在实心块和单向木板接缝上按下加跳会把木板记成「正在穿」、角色却没往下掉，之后落回木板直接穿过去：脚下踩着实心东西时不穿；站到别的东西上时清掉记着的对。
- 箱子刚好擦过单向平台顶面时又被推下去：「穿出来了」按包围盒和平台顶面的几何关系算，那一帧还用旧法线的接触改成朝上。
- 无头测试按帧数等异步加载，机器慢时偶尔没加载完就开始断言：改成按时间等（最多 15 秒，每帧睡 1 毫秒）。平台跳跃试验场、特效示例、3D 离屏渲染的测试都改了。
- 建图在大地图上很慢（400 × 60 格要一百多毫秒）：按列索引、同一段地面只试一次、跳过有遮挡的落点和整段空的格子，降到十几毫秒。
- 两张地图都盖住走地怪时总用第一张：用脚下有能站的地方的那张，地图范围也查竖直方向。
- 走地怪站在坡上时 `see` 按导航网格查，坡挡住视线：在它的地面地图上查，斜坡、单向平台不挡。
- TSX 没法让角色穿下去：加了 `dropThrough` command 和事件。
- 平台跳跃的「下」只有键盘：手柄左摇杆往下、触屏摇杆往下推也算；第二个角色也能穿下去。
- 编辑器打开旧关卡（图集 `[3, 1]`、没有规则）刷上新瓦片后存不下来：打开时按现在的图集升级，瓦片栏只列图集里有的帧。
- 按 R 重开时走地怪还在原地、树接着跑：id 带轮数，重开就重建。跳不上去的连线反复试：同一条失败 3 次就先不用；地面寻路按精灵的真实大小查碰头。
- 叠着的两行单向平台，寻路以为只穿一层、物理一次穿两层，落点对不上：两边都一次穿过连着的几层。
- 一条坡中间有一格下面是空的时整条都按悬空处理：拆成下面填满的最长几段，各自并成一块。
- 点地图时空格也算点中，挡住后面的精灵；瓦片地图和 z < 0 的精灵的先后不按 z：空格不算，按 z 排。
- 走地怪跟着角色进出视线时每次都喊：一次看见只喊一次。
- 斜坡帧又写进 `oneWay` 时不说：warn 一次。
- 改了瓦片规则时动画瓦片从第一帧重来：按场景时钟接着播。
- 叶子被打断时游戏没法收尾：加了 `tick_halting` / `halt_with` 的回调。
- `parallel` 的 `succeed` 比孩子多、`blackboard` command 没写 `value` 时不说：warn。
- 地面寻路不管坡多陡都算能走：按 `max_slope`。
- 树的时钟用 `f32`，跑一天以后冷却、等待的精度不够：改成 `f64`。

## 不做

- Tiled 的瓦片属性（单向平台、斜坡、图块动画）：Tiled 关卡还是只认 `solid`。
- 斜坡只到 22.5°（两格升一格），更缓的（三格、四格一组）和天花板上的倒坡不做；斜坡格之间的「接不上」（坡顶对着高一格的墙）不自动修，关卡自己摆对。
- 单向平台只挡 +y 一个方向；不做单向的墙（只能从一边穿过去）。
- 地面寻路不管移动平台、箱子这类动的东西；不算跳的时候在半空中拐弯；地图改了（换关）整张图在这一帧重建，不放到后台、不只重建改了的那一块；落点上面有遮挡（屋檐下面）的跳不试。
- 行为树不做子树引用、自定义装饰节点、事件驱动（每帧从根跑一遍）；场景里的叶子只有上面那几个，别的逻辑由游戏读 `behavior` 事件、写黑板去做。
- 动画瓦片不换碰撞（会开合的门这类要用精灵）；所有同一帧号的格子一起换帧，不错开相位。
- 联网时不同步行为树（见上）。
- 编辑器里不显示斜坡和单向平台的碰撞形状，也不能在编辑器里改图集的规则（规则跟着图集走）。

## 验收

- 还没手测：手感、画面（这台电脑上不开窗口）。石阶里走地怪追人、跳上单向木板、角色按下穿下去，都只在无头测试里跑过。
- `nge-physics2d`：
  - 单向平台：角色从下面跳上去、穿过的时候不算站着、落回来站住；按下穿下去落到地面、再跳上来又能站；从一块走出边落到下面一块；动态刚体（箱子）往上穿过、落回来停住、`drop_through` 后穿下去；kinematic 刚体从上面压下来照样被挡；头顶贴着平台底时 `drop_through` 不算。
  - 斜坡：`Shape::Slope` 是四边形（右下实心、左上空、射线打在坡面上、法线对）；两边都是 0 的不生成；45° 和 22.5° 的瓦片坡（每格一块，下面填实心）上坡、下坡每帧都在走（没卡住）、一路贴地；陡坡坡脚 40° … 49°、三种大小和速度都走得上去。
- `nge-scene`：
  - 单向平台瓦片和斜坡瓦片各自一块碰撞体，同一行连着的单向平台并成一块、上下两行不并；角色从下面跳上单向瓦片站住；斜坡瓦片走上走下不卡（并好的坡）；精灵刚体的 `oneWay`。
  - `tile_slopes`：45° 台阶并成一块、22.5° 跨行接上、下坡和悬空的坡、坡度不同的不并。
  - 动画瓦片只换帧不重建；材质按摩擦分开合并、冰面上箱子滑得远；点地图报格子和帧号；写错的规则只丢那一项；斜坡挡会飞的、单向平台不挡。
  - 行为树：走地怪跳上单向平台追到目标、目标回到地上时落下来追到、目标在平台正下方时从平台上穿下去追到；追不到时 `chase` 失败、选择节点改去 `emit`；巡逻（来回走）、看见就追、追丢了回巡逻线，黑板跟着变；`blackboard` command 写了才走；写错的树只丢 `behavior`；每帧重新声明同一个场景不重置树。
- `nge-ai`：
  - 地面寻路：站的地方；走上 45° 坡、坡顶接平地、反过来下坡；一格高的台阶要跳；从边上落、从单向平台穿下去、跳不上去的没有连线；跳的连线按能力生成（高 3 跳得上两格高的单向平台、高 1.5 跳不上，远 3 跳不过 4 格的空隙，能力越大连线越多）；天花板、墙挡住的跳没有连线，从天花板缺口下面跳得上去；路径里有跳、回来只用落和穿、一步接一步；能力小时找不到路；`Reach::from_physics` 的换算；`spot_below`。
  - 行为树：顺序遇到失败停、接着跑在跑的那个；反应式顺序每帧重查条件；选择取第一个成功的、反应式时高优先级打断低优先级（被打断的等待从头等）；并行凑够才成、凑不够就失败、成了的不再跑；取反；重复（次数、一直、孩子失败）；冷却（跑完开始、失败也冷却、在跑的时候不算、被打断不清）；等待；黑板共用、取值；和 `Fsm` 一起用。
- 平台跳跃：
  - Rust：从下面跳上单向木板站住，只按下不穿，按下加跳穿下去，地上按下加跳照常跳；单向木板上的走地怪下来撞到角色、角色回到起点。
  - TSX：第二关的数量（2 个敌人、1 个走地怪）和走地怪的位置；第二关的瓦片规则从图集抄过来、用到的帧都在图集里；走地怪的行为树结构、每次生成的一样、跳得够高；关卡文件写成文本再读回来走地怪和瓦片规则都在。
  - 编辑器：瓦片层的规则读写一样、`toTilemap` 带上、`emptyLevel` 抄过来；写错的规则报出是哪一项。
- 变异检查（改坏一处、确认有测试挂、改回来）：

  | 改坏的地方 | 挂的测试 |
  | --- | --- |
  | 角色控制器的过滤不查「整个在平台上面」 | `characters_jump_up_through_and_stand_on_one_way_platforms`、`characters_drop_through_and_land_again_from_above` |
  | 角色控制器移动时不过滤单向平台 | 同上 2 个 |
  | 钩子不分方向（从下面来的也留着接触） | `dynamic_bodies_pass_up_through_and_rest_on_one_way_platforms` |
  | `drop_through` 不查是不是在平台上面 | `heads_touching_a_platform_from_below_do_not_drop_through_it` |
  | 分开了也不从「正在往下穿」里删 | `characters_drop_through_and_land_again_from_above` |
  | `drop_through` 不叫醒睡着的刚体 | `dynamic_bodies_pass_up_through_and_rest_on_one_way_platforms` |
  | 斜坡格不并成一块 | `tile_slopes` 的 3 个、场景 `slope_tiles_are_walked_up_and_down` |
  | 动画瓦片不挂帧动画 | 场景 `animated_tiles_change_frames_without_rebuilding_the_map` |
  | 材质不按帧号找 | 场景 `ice_lets_boxes_slide_further` |
  | 点格子时行列反了 | 场景 `tapping_a_tilemap_reports_the_cell` |
  | 斜坡不挡会飞的 | 场景 `slopes_block_flyers_but_one_way_tiles_do_not` |
  | 精灵刚体的 `oneWay` 不传给碰撞体 | 场景 `sprite_bodies_can_be_one_way_platforms` |
  | 跳的连线不要求最高点高出落点 | `jump_links_follow_the_jump_ability` |
  | 跳的轨迹不查实心格 | `jumps_do_not_pass_through_ceilings_or_walls` |
  | 只试满高的跳 | 同上 |
  | 往下落时单向平台不接住 | 同上 |
  | 没有从单向平台穿下去的连线 | `falling_off_edges_and_dropping_through_one_way_platforms` |
  | 反应式的顺序 / 选择不从头查 | `reactive_sequences_recheck_conditions_every_tick`、`selectors_take_the_first_success_and_higher_priorities_interrupt` |
  | 被打断时清掉冷却 | `cooldown_blocks_the_child_for_a_while_after_it_finishes` |
  | 孩子失败不开始冷却 | 同上 |
  | 高优先级成了不打断后面在跑的 | `selectors_take_the_first_success_and_higher_priorities_interrupt` |
  | 并行凑不够了也不失败 | `parallels_succeed_when_enough_children_do` |
  | 等待从这一帧的结尾算 | `wait_succeeds_after_its_time`、`selectors_take_the_first_success_and_higher_priorities_interrupt` |
  | 走地怪不起跳（只写水平速度） | 场景 `walkers_jump_onto_platforms_and_drop_back_down_to_reach_the_target` |
  | 走地怪不从单向平台穿下去 | 同上 |
  | 精灵别的属性变了也重建行为树 | 场景 `behaviors_survive_redeclaring_the_same_scene` |
  | 找不到路也算在跑 | 场景 `unreachable_targets_fail_and_the_tree_moves_on` |
  | `behavior` 不单独解析（写错丢整个精灵） | 场景 `the_blackboard_command_and_bad_trees` |
  | 平台跳跃按下加跳不穿下去 | `jumps_up_through_planks_and_drops_through_with_down_and_jump` |
  | 关卡文件不读瓦片层的规则 | `level.test.tsx` 的 2 个 |
  | 站在实心块和木板接缝上也穿 | `standing_on_the_seam_of_a_solid_block_and_a_plank_does_not_drop` |
  | 着地了也不清掉正在穿的对 | `landing_on_something_else_forgets_the_drop` |
  | 不一起穿紧挨着的下一层木板 | `dropping_through_stacked_planks_passes_the_ones_right_below` |
  | 叠着的木板不按合起来的范围删 | 同上 |
  | 擦过顶面时不改旧法线 | `boxes_barely_clearing_a_platform_land_on_it` |
  | 寻路只穿一层单向平台 | `dropping_through_stacked_one_way_rows_lands_below_all_of_them` |
  | 不管坡多陡都能站 | `slopes_steeper_than_the_walker_can_climb_are_not_walkable` |
  | 跳的检查用固定大小 | `jumps_check_the_walker_size` |
  | `find_near` 取最远的 | `heading_for_a_point_over_a_pit_stops_at_the_nearest_edge` |
  | 不跳过要避开的连线 | `avoided_links_are_not_used` |
  | 地图视线把单向平台也当挡 | `ground_maps_see_over_slopes_and_through_one_way_rows` |
  | 跳的检查不跳过整段空格（建图变慢） | `building_a_big_map_stays_fast` |
  | 打断时不调收尾 | `halted_leaves_get_a_callback` |
  | 树的时钟按 `f32` 累加 | `the_clock_keeps_precision_after_running_for_days` |
  | `chase` 失败也留着目标 | 场景 `a_failed_chase_does_not_keep_walking_toward_the_target` |
  | 目标悬空时不找最近的地方 | 场景 `targets_over_a_pit_are_chased_to_the_edge_without_falling_in` |
  | 没路时走出坑边 | 场景 `walkers_without_a_plan_do_not_walk_off_ledges` |
  | 掉出地图不管 | 场景 `walkers_that_fall_out_of_the_map_go_back_home_and_say_so` |
  | 失败多少次都不放弃 | 场景 `walkers_give_up_on_jumps_they_keep_failing` |
  | 总用第一张地图 | 场景 `walkers_use_the_map_under_their_feet` |
  | `see` 不用地面地图 | 场景 `walkers_standing_on_slopes_still_see` |
  | `parallel` 永远成不了不 warn | 场景 `trees_that_can_never_succeed_and_commands_without_a_value_are_flagged` |
  | 走地怪大小不从精灵来 | 场景 `walker_size_and_slope_come_from_the_node` |
  | 一条坡有一格悬空就整条拆开 | `falling_slopes_and_floating_ones` |
  | 空格也算点中；瓦片地图总在后面的精灵前 | 场景 `tapping_sorts_tilemaps_and_background_sprites_by_z_and_skips_empty_cells` |
  | 斜坡帧又写进 `oneWay` 不说 | 场景 `slope_frames_also_listed_as_one_way_are_flagged` |
  | 换规则时动画从头播 | 场景 `animated_tiles_change_frames_without_rebuilding_the_map` |
  | `dropThrough` command 不穿 | 场景 `the_drop_through_command_drops_characters_off_one_way_platforms` |
  | 第二个角色按下加跳不穿；左摇杆往下不算「下」 | 平台跳跃 `the_stick_pushed_down_and_the_second_player_drop_through_planks_too` |
  | 摇杆往下的阈值反了；摇杆不发「下」 | `touch.test.tsx`、`touch-widgets.test.tsx` |
  | 走地怪 id 不带轮数；巡逻时不清喊过的记号 | `levelfile.test.tsx` |
  | 旧关卡不升级 `grid`；调色板不按图集过滤 | 编辑器 `store.test.tsx` |

  第一轮有 5 处没挂：跳的高度余量、矮跳、下落时被单向平台接住、打断时清冷却、精灵别的属性变了也重建行为树，各补了测试（最后一处原来的测试每帧声明同一个场景，同步时整个节点没变就跳过，没测到）。重复节点在孩子成了以后再打断一遍孩子：孩子跑完自己就回到没开始的样子，去掉了这一步。

  「修过的问题」那一轮的变异都挂了。建图的提速是几处一起的：单去掉「同一段地面只试一次」或「按列取候选」时别的几处还兜得住，测试不挂；去掉「整段空格直接跳过」时挂。
