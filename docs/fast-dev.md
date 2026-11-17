# 开发版也开优化：依赖全开、自己的代码轻度

> 前置：生成的宿主（[generated-hosts.md](generated-hosts.md)）、性能面板（[perf-overlay.md](perf-overlay.md)）。

## 目标

`nsh run` 把游戏的 Rust 代码按调试版编译，整个仓库也没配过任何 profile。所以 wgpu、naga、bevy_ecs、rapier、glyphon 全都不优化，平台跳跃一帧光 CPU 就要十几毫秒，60 帧都跑不满。发布版（`nsh package`）没有这个问题，可开发时一直在看的正是调试版。

**验收标准**：
- `nsh run` 和 `cargo test` 下，依赖全开优化，自己的代码轻度优化，帧时间接近发布版。
- 改了游戏或引擎代码后，增量编译还是几秒。
- 有一个基准能随时量一帧在 CPU 上花多久。

## 做法

### 开发配置（`.cargo/config.toml`）

- `[profile.dev] opt-level = 1`：自己的代码轻度优化，调试信息和 `debug_assert!` 照常。
- `[profile.dev.package."*"] opt-level = 3`：依赖全开。
- 写在 `.cargo/config.toml`，不写在 `Cargo.toml`。
  - `nsh run` 编的是它在 `examples/*/.nsh/rust` 生成的工作区，那里的 `Cargo.toml` 每次都重写。
  - cargo 从当前目录往上找这份配置，所以 `nsh run`、`cargo` 要在仓库目录里运行。`[patch.crates-io]` 早就靠这一点放在这里。
- 自己的 crate 在 nsh 生成的工作区里不是成员，也会被 `"*"` 匹配上，开到 3。所以每个 crate 都单独写一节压回 1。
  - 测试 `dependencies_are_optimized_and_own_crates_stay_light` 检查 `crates/*`、`examples/*` 里每个 crate 都写了，也不留已经删掉的 crate。
  - 按包名匹配，不看来源：依赖里要是有同名的包（示例叫 `model`、`physics` 这类通用名），它也会被压成 1，只是慢一点。

### 基准（`nge-scene/tests/frame_bench.rs`）

- 场景像平台跳跃：一张 200 列的瓦片地图、300 个刚体、100 个带着色器的精灵、10 段文字、2D 光照、一遍暗角。
- 先跑 60 帧热身，再跑 300 帧，分别统计更新和渲染的耗时：平均、中位、p95、最长。
  - 渲染只算 CPU 录制和提交命令，不含 GPU 画的时间。
  - 热身之后刚体大多已经落地静止，物理量的是静止时的开销。
  - 跑完检查场景里的精灵真的加载出来了，开头打印用的是哪块 GPU。
- 默认不跑：

  ```bash
  cargo test -p nge-scene --test frame_bench -- --ignored --nocapture
  ```

### 量出来的结果（Mac，M 系列）

| 配置 | 更新（平均 / p95） | 渲染（平均 / p95） |
| --- | --- | --- |
| 调试版（以前） | 6.62 / 13.43 ms | 1.05 / 1.25 ms |
| 开发配置（现在） | 0.15–0.19 / 0.27–0.32 ms | 0.13–0.28 / 0.15–0.23 ms |
| 发布版 | 0.14 / 0.26 ms | 0.13 / 0.17 ms |

- 开发配置跑了两遍，取范围。渲染偶尔有一帧 30 ms 上下（驱动的长尾），会把平均值拉到比 p95 还高。

- 换配置后第一次要把依赖全部重新编一遍：仓库里的测试大约 1.5 分钟，平台跳跃的宿主大约 3 分钟。之后的增量编译：
  - 改游戏代码，重编平台跳跃的宿主约 2 秒。
  - 改一行引擎代码约 3 秒。如果引擎也按 3 优化，要 70 秒。

### 发布

- `nsh package --target macos` / `ios` 本来就按发布版编 Rust（`--release`），不用改。
- `nsh run --release` 只压缩 JS、去掉调试信息，不改 Rust 的编译方式。

## 不做

- 发布版的 LTO、`codegen-units = 1`：要等包体和启动时间成了问题再说。
- 让 nsh 在生成的工作区里带上应用自己的 profile（要改 nsh 上游）。

## 验收

- 测试：开发配置里依赖是 3、自己的代码是 1，每个 crate 都单独压回 1。
- 变异检查：漏掉一个 crate、依赖不优化、自己的代码不优化，都有测试报错。
- 用 `cargo build -v` 编平台跳跃的宿主工作区：wgpu_core 是 `opt-level=3`，nge_render2d、platformer 是 `opt-level=1`。
- 还没手测：用 `nsh run` 跑平台跳跃，按 P 看面板上的更新、渲染比以前小多少，体感顺不顺。

## 修过的问题

- 起初写在 `Cargo.toml` 里，`nsh run` 用不上。挪到了 `.cargo/config.toml`。
- 在 nsh 的工作区里引擎 crate 也被开到 3，改一行引擎代码要重编 70 秒。改成每个 crate 单独压回 1。
- 检查配置的测试只认紧跟在节标题下的一行，包名取的是文件里第一个 `name =`；改成按节读到下一个节为止（容忍 CRLF、多个键），包名只从 `[package]` 里取，并检查没有已删除 crate 的残留节。
- 基准可能在量一个没加载出来的空场景；加了精灵数的检查。
