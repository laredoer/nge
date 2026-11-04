# 音频

> 前置：资源层（[m5-extensions.md](m5-extensions.md)）、TSX 场景节点（[scene-nodes.md](scene-nodes.md)）。

## 目标

按 `asset:/` 路径加载音效和音乐（wav / ogg / mp3），在 Rust 里播放、循环、调音量，TSX 里用一条 command 播音效、在 `scene` 里声明背景音乐。

**验收标准**：`examples/physics` 加上声音，资源都写 `asset:/` 路径：
- 有一段循环的背景音乐，HUD 上有静音按钮；
- 弹球撞到东西时「叮」一声（音高随球的大小变），点中弹球时「啵」一声；
- 一次加 200 个球、碰撞很多时不卡顿，同时发声的数量有上限；
- 没有音频设备时（CI、测试）游戏照常跑，只记一条 warn。

## 新 crate `nge-audio`

用 kira 0.12（cpal 输出，Mac / iOS 都支持；自带 wav、ogg、mp3 解码），只开需要的 feature。

- **资源 `Sound`**：`server.load::<Sound>(uri)`，在资源层的后台线程上解码成 PCM（`StaticSoundData`）。音乐也用它（整段解码进内存；几分钟的 ogg 解出来是几十 MB，够用了，流式播放放到后面）。
- **资源 `Audio`**（`Resource`）：
  - `play(sound, Play { volume, pitch, pan, looped })` → `Voice`。声音还没加载完就先排队，加载完的那一帧开始播；加载失败记 warn 丢掉。
  - `stop(voice, fade)`、`set_voice_volume(voice, volume, fade)`；`set_volume(总音量)`、`set_muted`。
  - 音量是线性的（0 = 静音，1 = 原音量），内部换成分贝交给 kira；`pitch` 是播放速度（2 = 高八度），限制在 0.1 到 8（太快会卡住音频线程，太慢的声音会一直占着上限），不是正数时用 1；`pan` 是 NaN 时居中。
  - 同时发声的数量有上限（默认 32），超出时新的音效直接丢掉（循环的音乐不受限制）。一帧里同一个声音最多开始 4 次，碰撞多的时候不会叠成一片（循环的也不占这个次数）。`stop` 之后正在淡出的声音不再算进上限，`is_playing` 也马上变成 false。
  - 播完的声音每帧清理。同一个加载失败的声音只 warn 一次。
- **后端**：`AudioPlugin::default()` 用 cpal；创建失败（没有音频设备）时记一条 warn，之后所有调用都是空操作（`play` 不排队；`Audio::enabled()` 可以查，`scene.audio` 和 `sound` command 在没有设备时连加载都不做，声音也就不去解码）。测试用 `AudioPlugin::mock()`（kira 的 `MockBackend`，不出声，可以数正在播的声音）。
- 系统排在 `PostUpdate`：游戏在 `Update` 里调 `play`，这一帧就能开始。

## TSX

- 命令 `sound`：`{ src, volume=1, pitch=1, pan=0 }`，播一次音效。TS 里用 `sound(node, …)`。
- `scene.audio`：`{ music, musicVolume=1, volume=1, muted=false, preload: [] }`。
  - `music` 是 `asset:/` 路径，循环播放；换了就把旧的在 0.5 秒里淡出、新的淡入；去掉就淡出停止。加载失败的音乐不会重试，换一个路径（或者先去掉再加回来）才会重新播。
  - `volume` / `muted` 是总音量；`preload` 里的声音提前加载，第一次播放时不用等。
  - 和别的声明一样，变了才覆盖。

## 实现上的决定

- `AudioManager` 放在 `Mutex` 里当普通资源（Mac 和 iOS 上 kira 的 cpal 后端都满足 `Send`，编译检查过）。
- wav 要开 kira 的 `wav` 和 `pcm` 两个 feature（前者是容器，后者是编解码器），只开 `wav` 会报 unsupported codec。
- 淡入淡出、音量过渡的时间最长 60 秒，最短 10 毫秒。
- 音乐的音量变化在 0.2 秒里平滑过渡；总音量 / 静音在 0.05 秒里过渡，避免爆音。
- TSX 示例里碰撞声自己先限流（100 ms 里最多 6 个），免得碰撞成片时往引擎塞一堆 command；引擎这边还有自己的上限。
- 示例的 Rust 测试用 `AudioPlugin::mock()`，`cargo test` 不出声。其他示例的测试里 `ScenePlugin` 会补一个真的 `AudioPlugin`，只打开设备、不放声音。
- `ScenePlugin` 没加 `AudioPlugin` 时自己补上（和粒子、文字、物理一样）。
- 示例的音效和音乐是用脚本合成的 wav（不从网上下载），脚本放在 `examples/physics/tools/`。

## 范围外

- 流式播放长音乐、空间音效（按位置算左右声道和衰减）、效果器（混响、滤波）
- 录音
- iOS 的 `AVAudioSession` 设置（静音开关、和别的 App 混音）用系统默认

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-audio` 10 个（wav 解码、坏文件加载失败、没加载完先排队 / 加载失败丢掉、播完清理而循环的一直在、排队中停掉就不播、一帧里同一个声音最多 4 次 / 同时最多 32 个 / 循环的不受限、线性音量换分贝和总音量 / 静音、没有设备时全是空操作且不排队、pitch / 过渡时间的边界、排队中改音量、循环的不占一帧里的次数）；`nge-scene` 新增 2 个（音乐开始、只改音量不重新开始、换成加载失败的音乐时旧的停掉、去掉 `audio` 就停；`sound` command 和 `preload`）；`examples/physics` 新增 1 个（背景音乐在放、`sound` 能播）。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过（含 kira 的 cpal 后端）；没上真机。
- **TS**：`nsh check` 通过 `examples/physics`、`examples/scene3d`、`examples/scene`。
- **试听**：在 Mac 上跑 `examples/physics`，确认能听到声音。
