# 声音补齐：3D 空间音频、跟着实体走、多普勒、遮挡、效果器链

> 前置：音频（[audio.md](audio.md)）、音频补齐（[audio-more.md](audio-more.md)）、3D 物理（[physics3d.md](physics3d.md)）、物理（[physics2d.md](physics2d.md)）、TSX 场景节点（[scene-nodes.md](scene-nodes.md)）。

## 目标

[audio-more.md](audio-more.md) 留下的几项：按位置的声音只有 2D 的、只在开始时定位置；没有多普勒和遮挡；每个分组固定一个低通一个混响。3D 游戏里的篝火、瀑布、绕着飞的东西、墙后面的声音都做不了。

**验收标准**：

- 声音可以带 3D 位置：按听的人（默认 3D 相机）的位置算衰减（线性、反比、指数，带最近、最远距离），按听的人的朝向算左右，可以带锥形指向。2D 的按位置播放不变。
- 场景里声明声源（`scene.audio.emitters`）：固定在一个位置，或者跟着一个精灵 / 模型走；有就响，没了就停；跟着的实体没了按配置停下或者播完。
- 多普勒：按声源和听的人的相对速度变调，有上限，瞬移不出怪声，默认关。
- 遮挡：听的人和声源之间有墙时压低音量、加低通，平滑过渡；射线查询有节流。
- 每个分组可以声明一串效果器（低通、高通、混响、延迟、压缩、限幅），参数平滑地变；可以往一条公用的混响总线上送。原来的 `lowpass`、`reverb` 照旧。
- 没有音频设备时全是空操作；算法都是纯函数，不出声就能测。
- 平台跳跃的 3D 试验场：一个绕圈的发声体（左右、多普勒）、一个墙后面的声源（遮挡）、一堆篝火（环境声）、按 U 切换水下的效果器链。

## 做法

### 纯函数（`nge-audio` 的 `spatial3d.rs`）

| 函数 | 算什么 |
|---|---|
| `attenuation(rolloff, distance, min, max, factor)` | 距离衰减，0 到 1。`min` 以内是 1，`max` 以外是 0 |
| `pan3(listener, right, source)` | 左右声像：声源方向在听的人右手方向上的投影，-1 到 1；重合时是 0 |
| `cone_gain(cone, forward, source, listener)` | 锥形指向：内锥里 1，外锥外 `outer_gain`，中间线性过渡 |
| `doppler(source_velocity, listener_velocity, source, listener, speed_of_sound, factor)` | 变调的倍数，截到 0.5 到 2 |
| `velocity(previous, now, dt, limit)` | 两帧位置差算速度；比 `limit` 快的当瞬移，速度是 0 |
| `approach(current, target, dt, seconds)` | 按固定速度靠近目标（遮挡量的平滑） |
| `occlusion_mix(amount)` | 遮挡量 → 音量倍数和低通截止频率 |
| `pick_probes(ages, budget, interval)` | 这一帧给哪几个声音打射线（节流） |

- 衰减模型（`Rolloff`）：
  - `Linear`：`1 - (d - min) / (max - min)`。
  - `Inverse`（默认）：`min / (min + factor × (d - min))`。
  - `Exponential`：`(d / min) ^ -factor`。
  - 反比和指数到 `max` 还不是 0：最后 10% 的距离里线性收到 0，免得走出范围时一下没声。
  - 默认 `min` 100、`max` 2000（引擎的单位是 point，一米 100）、`factor` 1。参数不合法（不是有限数、`min` 不是正数、`max` 不比 `min` 大）时用默认值。
- 声像：kira 的声像本身就是等功率的（左右各乘 `sqrt`），这里只算 -1 到 1 的位置，不另做 HRTF（见「不做」）。
- 多普勒：`(c - factor × 听的人朝声源的速度分量) / (c - factor × 声源朝听的人的速度分量)` 的倒数关系按标准公式算；两边的分量各截到 ±0.9c，结果截到 0.5 到 2。`factor` 0 是关，1 是真实的，大于 1 是夸张（游戏里的速度比声速小得多，一般要夸张才听得出来）。
  - 声速默认 34300（point / 秒），`Audio::set_speed_of_sound` 改。
  - 速度是引擎按位置差算的，再做 0.1 秒的平滑；一帧里挪动得比 12000 point / 秒还快的当瞬移，这一帧速度按 0 算。
  - 变调用 0.1 秒过渡给 kira，不会「咔哒」。
- 遮挡：每个声音有一个遮挡量（0 到 1），按 0.25 秒从头到尾的速度靠近目标（被挡是 1，没挡是 0）。全挡时音量乘 0.35、低通 800 Hz；中间音量线性、截止频率按对数插值。

### 效果器（`dsp.rs`、`effects.rs`、`chain.rs`）

kira 的效果器要在建轨道时定下来，之后不能加减。所以自己写一个 kira 的 `Effect`（`ChainEffect`），里面是一串自己的 DSP 节点，每个分组的轨道上挂一个，链子怎么变都在它里面：

- `EffectSpec`：
  - `Lowpass { cutoff, q }`、`Highpass { cutoff, q }`：二阶滤波（RBJ 的系数），每倍频程 12 dB。
  - `Reverb { room, damping, mix }`：Freeverb（8 个梳状滤波加 4 个全通，左右错开 23 个采样），单声道输入。
  - `Delay { time, feedback, mix }`：带反馈的延迟。改 `time` 时读的位置平滑地挪（像磁带变速），不跳。
  - `Compressor { threshold, ratio, attack, release, makeup }`：前馈压缩，按左右里大的那个算包络，硬拐点。
  - `Limiter { ceiling, release }`：限幅，起音是立刻的，输出不会超过 `ceiling`。
- `EffectSpec::sanitize()`：每个参数截到合法范围，不是有限数的用默认值（表在 TSX 一节）。一条链最多 8 个，多的丢掉并 warn。
- `Chain`：纯 DSP，`Chain::new(specs, sample_rate)`、`process(frame)`、`retarget(specs, seconds)`。
  - 参数每 32 个采样走一小步（截止频率按对数走），系数跟着重算。
  - 采样率很小（测试的不出声后端是 1）时缓冲至少 1 个采样、截止频率截到采样率的 0.45 倍，不出 NaN。
- `ChainEffect`：
  - 链子的形状（种类和顺序）没变、只改了参数：发一条 `Retarget`，在 `fade` 秒里过渡过去。
  - 形状变了：在游戏线程上按音频线程报上来的采样率建好新的 `Chain` 发过去，新旧两条同时算、30 毫秒交叉淡变；换下来的旧链从另一条通道送回游戏线程释放，音频线程上不分配、不释放内存（采样率变了时除外）。
- 分组轨道上的顺序：原来的低通 → 原来的混响 → 这条链。
- 混响总线：启动时建一条 kira 的 send 轨道，上面也是一条 `ChainEffect`（默认是一个全湿的混响）；每个分组的轨道建的时候就连过去，音量一开始是静音。`set_send(bus, amount, fade)` 调送多少（线性，0 到 1），`set_send_effects(specs, fade)` 换总线上的链。

### `Audio` 新增的接口

- `Play` 多两个字段：`at3: Option<Vec3>`（3D 位置；和 `at` 都写了时用 `at3`）、`emit: Emit`。
- `Emit { rolloff, min, max, factor, cone, forward, doppler, occlude }`：衰减、锥形和它的朝向、多普勒系数、要不要查遮挡。衰减和锥形只对 3D 的声音起作用（2D 的照旧按 `range`）；多普勒和遮挡 2D、3D 都管用。
- 听的人：`set_listener3(position, right)`（`right` 是右手方向）、`listener3()`。2D 的 `set_listener` 不变。
- 挪声音：`set_voice_position3(voice, position)`、`set_voice_forward(voice, forward)`；2D 的 `set_voice_position` 不变。
- 遮挡：`occlusion_probes()` 返回这一帧该查的 `Probe { voice, from, to, three_d }`（每帧最多 4 条，同一个声音至少隔 0.1 秒，等得最久的先查）；`set_occluded(voice, bool)` 把结果告诉它。`nge-audio` 不依赖物理，射线由场景层打。
  - 开了遮挡的声音各自有一条子轨道，上面一个低通；同时最多 16 条，多出来的只压音量、不加低通。
- 效果器：`set_effects(bus, specs, fade)`、`effects(bus)`、`set_send(bus, amount, fade)`、`send(bus)`、`set_send_effects(specs, fade)`、`send_effects()`。
- 调试和测试用：`rate(voice)`（现在的播放速度，含多普勒）、`occlusion(voice)`（遮挡量）。

### 场景（`nge-scene`）

- `scene.audio` 多出：
  - `emitters?: SoundEmitterNode[]`：声明式的声源，按 `id` 增量同步——新出现的开始播，没了的淡出停掉，`src` 换了重新开始，音量变了平滑过渡，别的参数变了原地改。
  - `listener?: { follow, space? }`：听的人放在这个精灵（`space: "2d"`，默认）或模型（`"3d"`）上；不写时 2D 的在 2D 相机上、3D 的在 3D 相机上。3D 的左右一直按相机的朝向算（和画面一致）。
  - `speedOfSound?`、`send?: { effects? }`（混响总线上的链）。
  - `buses.*` 多出 `effects?`、`send?`、`fade?`（这个分组的设置变了时过渡多久，默认 0.3）。
- `SoundEmitterNode`：`id`、`src`、`loop`（默认 `true`）、`volume`、`pitch`、`bus`、`fade`（出现、消失时淡入淡出的秒数，默认 0.3），加上下面这组和 `sound` 命令共用的字段：
  - `space?: "2d" | "3d"`（默认 `"2d"`）、`x`、`y`、`z`。
  - `follow?: string`：跟着这个 id 的精灵（2D）或模型（3D）走，这时 `x`、`y`、`z` 是相对它的偏移（不跟着转）。每帧在物理、跟随、相机都走完以后更新。
  - `onLost?: "stop" | "finish"`：跟着的实体没了以后，`stop`（默认）淡出停掉，`finish` 留在最后的位置播完（循环的一直到声明去掉）。实体又出现时循环的声源重新开始。
  - `rolloff?`、`min?`、`max?`、`factor?`、`cone?: { inner, outer, outerGain?, direction? }`（度；不写 `direction` 时用跟着的模型的朝向，模型朝 -z）。
  - `doppler?: number`、`occlusion?: boolean`。
- 不循环的声源出现时播一次，播完不再播（`id` 换了才算新的）。
- `sound` 命令多出同一组字段：`space: "3d"` 时按 3D 位置播；带 `follow` 的一次性音效跟着实体走。原来的写法（`x`、`y` 都写了就按 2D 位置播）不变。
- 遮挡的射线（`PostUpdate`，音频更新之前）：从听的人打向声源，碰到固定刚体或者没有刚体的碰撞体算挡住；动态、kinematic 的刚体和 sensor 不算（穿过去接着查，最多穿 4 个），跟着的实体自己不算。2D 的用 2D 物理，3D 的用 3D 物理（没开 `physics3d` feature 时 3D 的不查，当没挡）。

### TSX

- `AudioNode.emitters`、`listener`、`speedOfSound`、`send`；`BusNode.effects`、`send`、`fade`；类型 `SoundEmitterNode`、`SoundSpace`、`AudioEffect`、`SoundCone`。
- `sound()` 的参数多出 `SoundSpace` 那一组。
- 效果器的写法和参数范围：

| `type` | 参数（默认，范围） |
|---|---|
| `lowpass` / `highpass` | `cutoff`（必写，20 … 20000 Hz）、`q`（0.707，0.1 … 10） |
| `reverb` | `room`（0.5，0 … 1）、`damping`（0.5，0 … 1）、`mix`（0.3，0 … 1） |
| `delay` | `time`（0.25 秒，0.001 … 2）、`feedback`（0.35，0 … 0.95）、`mix`（0.3，0 … 1） |
| `compressor` | `threshold`（-18 dB，-60 … 0）、`ratio`（4，1 … 20）、`attack`（0.01 秒，0.0001 … 1）、`release`（0.15 秒，0.001 … 5）、`makeup`（0 dB，-24 … 24） |
| `limiter` | `ceiling`（-1 dB，-24 … 0）、`release`（0.1 秒，0.001 … 5） |

### 示例（平台跳跃的 3D 试验场）

- 绕圈的发声体：一个小球绕着场地中间转（Rust 里挪），声源 `follow` 它，开多普勒（夸张 12 倍）。
- 墙后面的声源：一堵固定的墙，后面一个开了遮挡的声源；走到墙后面就清楚了。
- 篝火：一个指数衰减的环境声，边上一个 3D 粒子的火苗。
- 听的人跟着角色（左右按相机）。
- 按 U 切换「水下」：`sfx` 分组换成低通 500 Hz 加一点混响，再按回来（Rust 里调 `set_effects`，离开试验场时恢复）。
- 三段循环的音效是 `tools/make_sounds.py` 合成的。

## 不做

- HRTF（按头型的卷积）：要卷积和一套测量数据，依赖和 CPU 都重。左右用等功率声像；前后不分。
- 按材质的遮挡（木头透一点、石头全挡）、绕射、多条射线算部分遮挡：现在一条射线，挡没挡两种。
- 按房间的混响区域：混响总线只有一条，送多少由游戏自己按分组设。
- 每个声源各自的送出量：只有分组能往混响总线送。
- 2D 的声音用新的衰减模型：2D 的按位置播放保持原样（按 `range` 线性）。
- 流式播放、录音：和这一项无关。
- 效果器的旁路链、侧链压缩、参数均衡。
- TSX 里按键切换效果器链的专门命令：声明式地改 `buses.*.effects` 就行。
