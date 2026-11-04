# 手柄（Mac）

> 前置：键盘（[keyboard.md](keyboard.md)）。

## 目标

连上手柄就能玩：按钮和摇杆方向当成按键，已有的按键逻辑加几个键名就能用手柄；摇杆的模拟量单独给，做「推一半走得慢」。

**验收标准**：`examples/platformer` 用手柄的十字键 / 左摇杆跑、A 跳；`examples/dodge` 用左摇杆按推的力度左右移动；切到别的窗口时按着的手柄键全部算松开。

## 键名

手柄的按钮和摇杆方向走键盘那条路（`key` command、`Input::key_down` / `axis`、TSX `onKey`），键名都以 `Gamepad` 开头，和键盘的不会重名：

| 键名 | 是什么 |
|---|---|
| `GamepadA` / `GamepadB` / `GamepadX` / `GamepadY` | 右边四个键，按 Xbox 的位置：A 在下、B 在右、X 在左、Y 在上（PlayStation 的 ✕ ○ □ △，Switch Pro 的 B A Y X） |
| `GamepadUp` / `GamepadDown` / `GamepadLeft` / `GamepadRight` | 十字键 |
| `GamepadLeftStickUp` … `GamepadLeftStickRight` | 左摇杆往这个方向推过一半 |
| `GamepadRightStickUp` … `GamepadRightStickRight` | 右摇杆，同上 |
| `GamepadL1` / `GamepadR1` | 肩键 |
| `GamepadL2` / `GamepadR2` | 扳机按过一半 |
| `GamepadL3` / `GamepadR3` | 按下摇杆（手柄没有就不会出现） |
| `GamepadMenu` / `GamepadOptions` | 菜单键（Xbox ≡ / PS Options / Switch +）、选项键（Xbox ⧉ / PS Share / Switch −；没有就不会出现） |

- 手柄按键不会自动重复（`repeat` 总是 false）。
- 摇杆方向「推过一半」是看那一根轴的值 > 0.5 或 < −0.5；斜着推两个方向都算按着。
- 系统的 Home 键（Xbox 徽标、PS 键）留给系统，不转发。

## 摇杆模拟量

`Input::stick(Stick::Left | Stick::Right) -> Vec2`：x 向右、y 向上，各在 −1 … 1，有死区（长度 < 0.15 时是 0）。平台层在值变了的时候发内置 command `gamepad { left: [x, y], right: [x, y] }`，引擎记进 `Input`。TSX 不收摇杆模拟量（每帧都在变，过 NSH 只会白白占主线程）。

扳机的模拟量不给：需要时再加。

## 什么时候转发

- 平台层在 display link 的每一帧（主线程）读一遍所有已连接手柄的状态，和上一帧比，变了的按下 / 松开转发。不用 GameController 的回调：每帧读一次已经够，也不用管回调在哪个线程。
- 连了几个手柄时合起来算：任何一个按着就算按着；摇杆取推得最远的那个。一个人玩的游戏不用关心手柄是哪个，多人分手柄需要时再加。
- 和键盘一样，游戏视图所在窗口不是 key window、视图（NSView）被隐藏（比如导航切走了这一屏）、没在窗口里时不转发；这时候按着的手柄键全部补发松开，摇杆归零。TSX 用样式 `hidden` 藏起来的游戏视图 nsh 0.3 只藏了 layer，检测不到，照样转发（键盘也一样）。TSX 输入框正在打字时手柄照样转发（手柄不会往输入框里打字）。
- 手柄拔掉时它按着的键松开（下一帧读不到它了，按「没按」算）。
- 手柄事件本来就不经过 AppKit 的事件队列，没有「吞键」的问题，`captureKeys` 和它无关。
- 同一个窗口里有几个游戏视图时每个都收到手柄。

## 实现上的决定

- 新文件 `crates/nge-platform/src/pad.rs`：手柄一帧的读数 `Reading`（按钮按着没有、摇杆和扳机的值）→ 按着的键名、死区后的摇杆，几个读数合并，和上一帧比出要转发的松开 / 按下 / 摇杆，都是纯函数，单元测试覆盖。
- 补发松开有四处：每帧读数、任何窗口 resign key 的通知、视图离开窗口、视图被隐藏（`viewDidHide`，display link 这时可能不回调）。`macos.rs` 只负责从 `GCController` / `GCExtendedGamepad` 读数。
- 只认 `extendedGamepad`（现在的手柄都有）；老的 micro gamepad（Siri Remote）不认。
- 引擎侧：`nge-core` 增加内置 command `gamepad`（保留名）和 `GamepadInput`；`nge-ecs` 的 `Input` 记两根摇杆，`App::command` 收它。
- iOS 也有 GameController，读数和合并逻辑都在 `pad.rs` 里，以后 iOS 只要在自己的 display link 里调一下；这次不做。

## 不做

- iOS / Android 的手柄、多人分手柄、震动、手柄灯、键位重映射、扳机模拟量、陀螺仪 / 触摸板。
- 「按住不放自动重复」：菜单要的话游戏自己按时间做。

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-core` 1 个（`gamepad` 的 JSON 格式、保留名）；`nge-ecs` 新增 2 个（摇杆限幅、NaN 当 0、帧末不清；`gamepad` command 进 `Input`、坏的丢掉）；`nge-platform` 新增 6 个（按钮和扳机门槛、十字键和摇杆方向、死区、几个手柄合并、坏的轴值、和上一帧比出松开 / 按下 / 摇杆）；`examples/platformer` 1 个（左摇杆 / 十字键跑、A 跳）；`examples/dodge` 1 个（按摇杆推的力度移动）。
- **修过的问题**：补了 `viewDidHide` 时松开、把「和上一帧比」挪成纯函数加测试、文档写明 TSX 样式 `hidden` 检测不到、TS 注释里键盘专属的规则标明只对键盘。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`、`examples/platformer`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过（手柄只在 macOS 上）。
- **实跑**：`platformer`（release + `nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接。没有真手柄，GameController 这层没有实测；手测时留意：后台期间松开的键回到前台会不会多发一次按下、蓝牙手柄在没打包成 .app 的程序里能不能读到。
