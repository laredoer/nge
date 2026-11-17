# 动画状态机：状态、转换、混合树、分层、叠加、动画事件

> 前置：动画（[animation.md](animation.md)）、3D 补齐（[3d-more.md](3d-more.md)）、3D 渲染补齐（[render3d-more.md](render3d-more.md)）、TSX 场景节点（[scene-nodes.md](scene-nodes.md)）、角色控制器（[character.md](character.md)）、触屏控制（[touch-input.md](touch-input.md)）。

## 目标

现在 3D 只有 `Animator::play(clip, fade)`：游戏自己判断什么时候播哪段，走和跑之间只能硬切或者淡入淡出，脚步对不上；上半身挥手、下半身走路这种分开的动作做不了；受击抖一下只能整个换掉当前动作；脚步声、攻击判定帧要游戏自己按时间猜。2D 的精灵帧动画（`anim`）更简单，只能循环一段帧。

这一项补上动画状态机：

- **状态机**：状态（放一个片段或一棵混合树）、转换（条件来自参数：bool、float、trigger；过渡时长；「播到多少才能转」的退出时间）、任意状态转换、默认状态。参数由游戏设，Rust 和 TSX 都能设。
- **混合树**：一维（按速度在待机、走、跑之间混）、二维（方向，梯度带插值）；混合时同步归一化相位，走和跑的脚步对得上。
- **分层**：多层叠加，每层有权重（数或参数名）和骨骼遮罩；层分覆盖（`override`）和叠加（`additive`）。
- **叠加动画**：以参考姿势为基准算出差值，叠到下面的层上（任何动作上叠一个受击抖动）。
- **动画事件**：片段的某个时间点触发（脚步声、攻击判定帧），发到 Rust（ECS 的 `AnimEvent` 消息）和 TSX（`animEvent` 事件）；循环、跳帧、倒放、混合时不漏发、不重复发。
- **2D 也能用**：状态机核心和骨骼无关，只输出「每层在播哪些片段、各自的权重和时间」；3D 按它混姿势，2D 按它选精灵帧。
- **TSX 声明**：精灵、模型节点上写 `animGraph`（定义）和 `animParams`（声明式参数），`animSet` command 设参数、发 trigger；定义可以从 JSON 文件 `import`。

**验收标准**：

- 平台跳跃的主角换成精灵图集，状态机按角色控制器的速度、是否着地、竖直速度在待机、走、跑（一维混合、相位同步）、跳、落、落地之间切；走跑的脚步声、落地声是动画事件，Rust 收到后放音效。
- 平台跳跃第一关的果冻（3D、变形动画）改用状态机：TSX 声明 `near` 参数，靠近时淡到 squish。
- 状态机求值、转换条件、退出时间、混合权重、相位同步、骨骼遮罩、叠加差值、事件在循环 / 跳帧 / 倒放 / 混合时的触发都有单元测试；已有的示例和测试不回归。

## 做法

### 新 crate `nge-anim`（和骨骼无关的核心）

只依赖 `bevy_ecs`（组件、消息的 derive）和 `serde`。

- `GraphDef`（`def.rs`）：声明，和 TSX 的 `AnimGraphDef` 一一对应，`serde` 读 JSON。`Graph::compile(&GraphDef)` 校验并换成下标（状态名、片段名、参数名都查好），不合法时返回中文错误（哪一层、哪个状态、哪条转换的哪个条件），不建状态机。
- `AnimGraph`（`graph.rs`，组件）：编译好的图（`Arc`，同一份定义的多个实体共用）、参数值、每层的运行状态、这一帧的输出。
  - `set_float` / `set_bool` / `trigger` / `reset_trigger`，名字不对或类型不对时返回 `false`（不 panic，调的地方记 warn）。
  - `step(dt, &durations) -> Vec<Fired>`：往前走一步，`durations` 是「片段名 → 时长（秒）」，3D 从模型的动画取、2D 从帧数和 fps 算。返回这一步触发的事件。
  - `output() -> &[LayerOutput]`：每层 `{ weight, additive, clips: [{ clip, time, weight }] }`，同一层的片段权重加起来是 1。
  - `state(layer)`：现在在哪个状态（转换中是目标状态）。
- 纯函数：`blend1d`、`blend2d`（混合权重），`crossed`（一段时间里经过了哪些事件）。都在 crate 里单测。
- ECS：`AnimEvent { entity, layer, state, clip, name }` 消息；`Animated` 系统集（3D、2D 推进状态机的系统都在里面，转给 TSX 的系统排在它后面）。

### 定义

```jsonc
{
  "params": { "speed": 0, "grounded": true, "jump": "trigger" },   // 数是 float，true / false 是 bool，"trigger"
  "clips": {                                                       // 片段的事件；2D 的片段在这里写帧
    "walk": { "frames": [2, 3, 4, 5], "fps": 8, "events": [{ "t": 0.0, "name": "step" }, { "t": 0.25, "name": "step" }] }
  },
  "start": "move",
  "states": {
    "move": { "blend": { "by": "speed", "clips": [{ "clip": "idle", "at": 0 }, { "clip": "walk", "at": 80 }, { "clip": "run", "at": 240 }] } },
    "jump": { "clip": "jump", "loop": false },
    "land": { "clip": "land", "loop": false, "to": [{ "to": "move", "exit": 1 }] }
  },
  "any": [{ "to": "jump", "when": "jump" }, { "to": "land", "when": ["grounded", "vy < -10"], "fade": 0.05 }],
  "layers": []                                                     // 有 layers 时顶层的 start / states / any 是第 0 层
}
```

- 状态：`clip`（片段名）或 `blend`（混合树），二选一；`speed`（默认 1，负数倒放）、`speedParam`（再乘上这个 float 参数）、`loop`（默认 true）、`to`（转换列表）。
- 混合树：`by` 是一个参数名（一维）或两个（二维），`clips` 每个是 `{ clip, at }`，一维的 `at` 是数，二维的是 `[x, y]`。
- 转换：`to`（目标状态）、`when`（条件，一条字符串或数组，全部成立才转）、`fade`（过渡秒数，默认 0 硬切）、`exit`（退出时间，归一化：1 是播完一遍；不写不等）。
- 条件字符串：`"grounded"`（bool 为真，或 trigger 被设了）、`"!grounded"`、`"speed > 10"`（`>`、`>=`、`<`、`<=`、`==`、`!=`，右边是数）。写错的参数名、对 trigger 取反、bool 和数比较都是定义错误。
- 层：`{ name, weight = 1 | "参数名", mode = "override" | "additive", mask?: ["节点名"…], reference?: "片段名", start, states, any }`。
- `clips` 里的事件时间 `t` 是秒（片段自己的时间）。

### 求值（每层每步）

1. **推进**：每个活着的「状态实例」（转换中会有几个）按 `dt × speed × speedParam / 有效时长` 推进归一化相位。混合树的有效时长是子片段时长按权重的平均，所有子片段共用一个相位（相位同步）：子片段的时间 = 相位 × 自己的时长。循环的相位不封顶，取小数部分采样；不循环的夹在 [0, 1]。时长未知或是 0 的片段：循环的相位不动，不循环的直接到 1。
2. **事件**：见下面「动画事件」。
3. **过渡进度**：过渡的进度 `s` 加 `dt / fade`；`s ≥ 1` 时只留目标实例。
4. **转换**：先看任意状态的（`any`，跳过目标就是现在状态的），再看现在状态的 `to`，按写的顺序，第一条条件和退出时间都满足的生效；用掉它条件里的 trigger。一步每层最多转一次。
   - 退出时间：这个状态进入以后实际播过的归一化长度（每步加上相位变化的绝对值，不循环的停住后不再加）≥ `exit`。到了以后一直算满足（不像 Unity 那样每一圈只看一次）。
   - 过渡中也照样看转换（从目标状态看）：可以打断。
5. **开始过渡**：记下现在每个实例的权重作为起点 `from`，目标的权重 = `from_目标 + (1 − from_目标) × s`，别的 = `from × (1 − s)`，加起来总是 1，所以中途再转画面也不跳。转到还在淡出的状态时接着用那个实例（相位接着走，和 `Animator::play` 换回旧动画一样）；转到新状态时新建实例，相位从 0 开始（速度是负数时从 1）。`fade` 是 0 时直接只留目标。实例最多 8 个，多了丢掉最轻的。
6. **trigger**：一直留着，直到被某条转换用掉，或游戏 `reset_trigger`。

### 混合权重（纯函数）

- 一维 `blend1d(at, x)`：`at` 按大小排好；`x` 在两个相邻点之间线性分给这两个，超出两头的全给最近的那个。
- 二维 `blend2d(at, p)`：梯度带插值（freeform cartesian）：`w_i = min_{j≠i} clamp(1 − (p − p_i)·(p_j − p_i) / |p_j − p_i|², 0, 1)`，再归一化。正好在某个点上时那个是 1；只有一个点时是 1；`p` 不是有限数时当原点。

### 动画事件

- 实例这一步从累计时间 `a` 走到 `b`（片段的秒，循环的不取模）。`crossed(events, dur, looping, a, b, fresh)`：
  - 正放：`a < t + k·dur ≤ b` 的每个 `k`（循环）都算一次；倒放：`b ≤ t + k·dur < a`。按经过的先后排。
  - 刚进入的实例第一步（`fresh`）把起点也算上：时间 0 的事件在进入状态时发一次。
  - 不循环的 `k` 只有 0，停在头尾后 `a == b`，不再发。
  - 跳帧（一步跨好几圈）每圈都发；一步最多算 8 圈，多出来的圈不发（引擎本来就把一帧的 `delta` 截到 50 ms，只有 Rust 自己调 `step` 传很大的 `dt` 时才会碰到）。
- **混合时谁的事件算数**：
  - 每层只有权重最大的那个状态实例发事件（一样大时新进来的赢）；它里面是混合树时，只有权重最大的那个子片段发（一样大时写在前面的赢）。相位同步，所以走和跑混的时候脚步只响一次，换到跑占多数后跑的脚步接着响。
  - 被淡掉的一方、权重小的子片段在这段时间里经过的事件不发，之后也不补发（取消掉的攻击不出判定）。
  - 每层各自发；层权重是 0 的层不发。
- 事件带层名、状态名、片段名、事件名。Rust：`MessageReader<AnimEvent>`；TSX：`animEvent` 事件 `{ id, layer, state, clip, name }`（只给有场景 id 的实体）。

### 3D（`nge-render3d`）

- `animate_graphs`（`PostUpdate`，`Animated` 集）：有 `Model3d`、`AnimGraph` 的实体，片段时长从模型的动画按名字取（找不到的 warn 一次），推进状态机，再按输出算姿势写进 `Pose`（没有 `Pose` 的补上）；`Animator` 的系统跳过有 `AnimGraph` 的实体。渲染器在有 `Animator` 或 `AnimGraph` 时用 `Pose`。
- 姿势（纯函数，`graph_pose.rs`）：
  - 一层：每个片段从不播动画的姿势开始采样，按权重加权：平移、缩放加权平均，旋转按和第一个同半球后加权求和再归一化（nlerp），变形权重加权平均。
  - 覆盖层：`local = mix(下面, 这层, 层权重 × 遮罩[节点])`（旋转 slerp）。
  - 叠加层：先算这层的姿势 `P` 和参考姿势 `R`（`reference` 片段在 0 秒，不写就是每个片段自己的 0 秒，按同样的权重混），`local.t += (P.t − R.t)·w`，`local.r = local.r · slerp(I, R.r⁻¹·P.r, w)`，`local.s *= lerp(1, P.s / R.s, w)`（`R.s` 某分量是 0 时那一分量当 1），变形权重 `+= (P − R)·w`；`w` = 层权重 × 遮罩。
  - 第 0 层不管模式都按覆盖、权重当 1 算（下面没东西）。
- 遮罩：`mask` 是节点名，名字对上的节点和它所有子孙是 1，别的是 0；不写是全 1。`Skeleton` 多了 `names`（glTF 节点的名字，没有名字是空串）。名字一个都对不上时 warn 一次。

### 2D（`nge-scene`）

- 片段在定义的 `clips` 里写 `frames`、`fps`（默认 10），时长 = 帧数 / fps。
- `animate_sprite_graphs`（`PostUpdate`，`Animated` 集，在物理之后：Rust 在 `Update` 里设的参数这一帧就生效）：没有 `Model3d` 的实体，推进状态机，取最上面一层权重 ≥ 0.5 的覆盖层（都不够时取第 0 层）里权重最大的片段，按 `floor(时间 × fps)` 取帧（不循环的夹在最后一帧），写进图集精灵的 `frame`。
- 2D 不混帧：叠加层、遮罩在 2D 里不起作用（定义里写了照样能编译，2D 忽略）。

### TSX

- `SpriteNode.animGraph` / `ModelNode.animGraph`：`AnimGraphDef`。同步时和上一次声明**按值比较**，一样就不动（接着播），变了才重建（从默认状态开始）；所以 TSX 每次算场景时新建一个值一样的对象也不会重建。定义不合法时记 warn，不建（精灵退回 `anim` / `frame`，模型退回 `animation`）。精灵同时写了 `anim` 时以 `animGraph` 为准（warn），模型同时写了 `animation` 也一样。
- `animParams?: Record<string, number | boolean>`：声明式的参数，和上一次声明不一样（或刚建好）时才写进去，所以 Rust 在中间设的值不会每次同步都被盖回去。
- `animSet(game, { id, params?, trigger? })`：`animSet` command，设参数、发 trigger（一个名字或数组）。没有这个 id 或这个实体没有状态机时 warn。
- 内置事件 `animEvent`。
- 定义放在模块顶层的常量，或者 `import HERO from "./hero-anim.json"`（JSON 资源，Rust 也能 `include_str!` 同一份 `GraphDef::from_json`）。不要放在每帧都变的条件里让节点时有时无（[touch-input.md](touch-input.md) 的规则）：节点被去掉再加回来就是新实体，状态机从头开始。

### 示例

- **平台跳跃**：
  - `tools/make_hero.py` 生成 `assets/hero.png`（8 × 2 格，每格 24 × 32：待机 2 帧、走 4 帧、跑 4 帧、跳、落、落地）和 `make_sounds.py` 新加的 `step.wav`、`land.wav`。
  - `hero-anim.json`：参数 `speed`、`vy`、`grounded`、`jump`；`move` 是一维混合树（待机 0、走 100、跑 260，相位同步），走、跑各在落脚的帧有 `step` 事件；`jump`（trigger 进，`vy < 0` 转 `fall`）、`fall`、`land`（着地进，`land` 事件，播完回 `move`）。`levels.ts` 的 `HERO` 是 `texture: hero.png`、`grid: [8, 2]`、`animGraph`（就是这份 JSON），`game.tsx` 的主角节点 `...HERO`。
  - Rust：`control` 起跳和踩敌人弹起时记一下，`animate_player` 发 `trigger("jump")`，每帧设 `speed = |vx|`、`vy`、`grounded`，按水平速度的方向把 `Transform2d.scale.x` 翻过来（面朝左右）；读 `AnimEvent`，主角的 `step` 放轻的脚步声、`land` 放落地声。
  - 果冻：`animGraph: JELLY_ANIM`（idle、squish，`near` 时淡过去），`animParams: { near }`。
- **长坂坡**：不改。它的逻辑全在 TSX 的纯函数里一步一步算（`player.ts` 的连击、`combat.ts` 的判定按动作时间算），换成引擎的状态机和事件要把判定挪到异步的事件回调里，纯函数的测试和确定性都没了，改动量太大。

## 不做

- 二维混合只做梯度带（freeform cartesian），不做 freeform directional（按角度和长度分开插值）。
- 转换条件只有「参数 比较 常数」的与；没有或、参数和参数比较。
- 退出时间到了以后一直满足，不按圈判断；转换的过渡曲线只有线性；不做惯性化（inertialization）过渡。
- 同步组（不同状态之间的相位同步，Unreal 的 sync group）：只有同一棵混合树里的片段同步。
- 根运动（root motion）。
- 2D 的帧混合、2D 的叠加层和遮罩。
- 状态机的可视化编辑、运行时调试面板。
- 模型节点从 `asset:/` 路径异步读状态机的 JSON：TSX `import` 或 Rust `include_str!` 已经够用。
- 长坂坡改用状态机（见上）。

## 验收

全量检查（按顺序一个一个跑）：

- `cargo test --workspace -j 4`：70 组，986 个通过，0 个失败（新 crate 也在 `.cargo/config.toml` 里压回 `opt-level = 1`，`dev_profile` 测试要求的）。
- `cargo test -p nge-scene --no-default-features -j 4`：145 个通过。
- 三个示例每个测试文件 `nsh test`：平台跳跃 16 个文件、长坂坡 9 个、编辑器 8 个，全部通过；三个 `nsh check` 都通过。
- `cargo check --target aarch64-apple-ios -p platformer -p sanguo -j 4`、`tools/check-wasm.sh`、`tools/check-android.sh`：通过。

新加的测试：

- `nge-anim`（28 个）：定义的错误、从 JSON 读、参数名和类型检查；bool / float 条件全部成立才转、trigger 用一次、一步只转一次、退出时间、任意状态先看且不转回自己；淡入淡出的权重、打断淡入时权重连续、回到正在淡出的状态接着它的时间、实例封顶；一维、二维混合的权重，混合树子片段相位同步，二维用两个参数；速度参数和倒放从结尾开始、输出时间循环或夹住、层的权重和模式；`crossed` 的正放、跨圈、倒放、不循环；事件一次只发一次、倒放倒着发、混合时只发最重的片段、淡入淡出时只发权重大的那边、进状态时 0 秒的事件发一次、层权重是 0 时不发。
- `nge-render3d`：`graph_pose` 的 8 个（混合的平移缩放变形和旋转、反号四元数、遮罩带子孙、覆盖按每个节点的 alpha、叠加按参考算差值、带遮罩和权重的覆盖层加叠加层、状态机接管 `Animator` 并发事件、找不到的片段和对不上的遮罩），`gltf_load` 的节点名字。
- `nge-scene`：`anim_graph` 的 7 个（精灵按状态机取帧且重新同步不重建、声明的参数只在变了时写、`animSet` 设参数和 trigger、事件到 TSX、不合法的定义退回 `anim` 且状态机优先、模型的状态机代替 `animation`、2D 取最上面够重的覆盖层和混合树里最重的片段）。
- 平台跳跃 Rust：`hero_animation_follows_the_controller`（站着待机、全速跑的帧和脚步事件、往左转身且停下保持朝向、跳 → 落 → 落地 → 回到走跑且落地事件一次）、`hero_steps_and_landing_play_sounds`、`sounds_are_bundled` 加了 hero.png、step.wav、land.wav 和 `...HERO`。
- 平台跳跃 TSX：`App.test.tsx` 的主角图集和状态机定义、果冻改用 `animGraph` / `animParams`。

变异验证（逐个改坏关键逻辑、跑对应的测试、确认失败、改回）：

| 改坏的地方 | 结果 |
| --- | --- |
| trigger 用过不清 | 测试失败 |
| 条件只要一个成立就转 | 测试失败 |
| 不等退出时间 | 测试失败 |
| 任意状态转换转回当前状态自己 | 测试失败 |
| 任意状态转换不先于状态自己的转换 | 测试失败 |
| 打断淡入时权重跳变（不从当前权重开始） | 测试失败 |
| 回到正在淡出的状态从头播 | 测试失败 |
| 实例不封顶 | 测试失败 |
| 倒放不从结尾开始 | 测试失败 |
| 淡入淡出时按当前状态发事件（不看权重） | 测试失败 |
| 混合时不取最重的片段发事件 | 测试失败 |
| 层权重 0 也发事件 | 测试失败 |
| 混合树的子片段不按权重同步相位 | 测试失败 |
| 进状态时 0 秒的事件不发 | 测试失败 |
| 倒放的边界和正放一样 | 测试失败 |
| 跨圈的事件只算一次 | 测试失败 |
| 一维混合权重不归一 | 测试失败 |
| 二维混合权重不归一 | 测试失败 |
| 四元数混合不对齐半球 | 测试失败 |
| 遮罩不带子节点 | 测试失败 |
| 叠加层不减参考姿势 | 测试失败 |
| 参考缩放是 0 时不按 1 算 | 测试失败 |
| 遮罩层不乘层权重 | 起初没抓到，`layers_mask_and_additive_shape_the_pose` 的覆盖层改成权重 0.5 后测试失败 |
| 每次同步都重建状态机 | 测试失败 |
| 声明的参数每次同步都写（盖掉 Rust 设的） | 测试失败 |
| 2D 选帧算上叠加层 | 测试失败 |
| 2D 不取最重的片段 | 起初没抓到，`sprite_frame_uses_the_top_heavy_override_layer` 加了混合树的情况后测试失败 |
| `animSet` 的 trigger 不发 | 测试失败 |
| 平台跳跃起跳、踩敌人弹起不发 jump trigger | 测试失败 |
| 主角不转身 | 测试失败 |
| 不设 `grounded` | 测试失败 |
| 脚步事件不出声 | 测试失败 |
| `game.tsx` 的主角不用 `...HERO`（换回纯色） | Rust 的 `sounds_are_bundled` 失败（`App.test.tsx` 只测 `HERO` 本身，抓不到） |

## 还没手测

这一项没有开窗口跑，也没有起模拟器，下面这些只有编译检查和单元测试：

- 平台跳跃主角的帧动画、走跑混合的观感、脚步声的节奏和音量。
- 果冻靠近时的淡入。
- 3D 的分层、遮罩、叠加只在单元测试里用合成的模型验证过，没有在真模型上看过。
