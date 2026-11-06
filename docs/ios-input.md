# iOS 的手柄和硬件键盘

> 前置：键盘（[keyboard.md](keyboard.md)）、手柄（[gamepad.md](gamepad.md)）、生成的宿主（[generated-hosts.md](generated-hosts.md)）。

## 目标

手柄和键盘现在只在 Mac 上有。iPhone、iPad 连上蓝牙手柄或者硬件键盘（iPad 的妙控键盘、蓝牙键盘）也应该能玩，游戏的按键逻辑不用改。

**验收标准**：
- iOS 上手柄的按钮、摇杆方向和 Mac 一样转成 `Gamepad…` 键名，摇杆模拟量走 `gamepad` command。
- 硬件键盘的键转成和 Mac 一样的键名（`KeyA`、`ArrowLeft`、`Space`…），引擎和 TSX 的 `onKey` 都收到。
- 按着 ⌘ 时按下的键不发（留给系统快捷键），和 Mac 一样。
- App 进后台、游戏视图离开窗口或被隐藏时，按着的键和手柄全部补发松开，摇杆归零。
- 平台跳跃在 iPad 上用键盘或手柄玩，数字键选关。

## 做法

### 都用 GameController

- **手柄**：和 Mac 一样，读数（`gc::read_pads`）、合并、比较（`pad.rs`）两边共用，iOS 只是在自己的 display link 里调。
- **键盘**：用 GameController 的 `GCKeyboard.coalescedKeyboard`（iOS 14 起），所有键盘合成一个。每帧先问 `isAnyKeyPressed`，有键按着时再按键名表逐个问 `buttonForKeyCode`。
  - 不用 UIKit 的 `pressesBegan`：那要游戏视图当第一响应者，会和 NSH 的输入框、导航抢焦点；GameController 不管谁是第一响应者。
  - 键码就是 HID 键盘用法码（`GCKeyCode` 的值），`hid.rs` 的表把它换成键名。表里的键和 Mac 的键名表一一对应，测试里两边对一遍，再拿 GameController 自己的常量对一遍。
  - GameController 不给系统的自动重复，`repeat` 总是 false（手柄也一样）。
- **⌘**：`hid::Keyboard` 记着转发过的键。按着 ⌘（左右都算）时新按下的非修饰键不发，一直到它松开；修饰键本身照发。和 Mac 有几处不同：
  - Mac 上按着 ⌘ 时系统的自动重复会被当成新按下；iOS 没有重复，只看按下那一刻。
  - 开始读之前（比如回到前台时）就按着的键，第一帧读到时当成刚按下。
  - 补发松开时（进后台、失去焦点、离开窗口）「被 ⌘ 压住的键」一起清掉，回来以后还按着的话照常发。

### 什么时候读

- 主线程的 display link 每帧先读输入，再采样视口、出帧。
- 只在 App 在前台、拿着焦点（active）、视图在窗口里、视图和它的父视图都没隐藏时读；不然当成什么都没按，按着的补发松开。
- App 进后台时（通知是在主线程收到的）马上补发松开：后台 display link 不一定还走。
- 拉下控制中心、来电横幅、分屏切走焦点时 App 没进后台，但会失去焦点（`willResignActive`）：这时也补发松开、停止读输入，渲染照常；`didBecomeActive` 后再读。
- 轮询每帧读一次，比一帧还短的点按（按下和松开都落在两帧之间）会漏掉。

### 转发

和 Mac 一样：按下、松开发 `key` command 给引擎，同时发 `key` 事件给 TSX；摇杆变了发 `gamepad` command。

## 不做

- TSX 输入框正在打字时不转发：iOS 没有公开的办法知道谁是第一响应者，打字时键盘照样转给游戏。游戏界面里放输入框时，用 TSX 自己判断。
- 吞键：GameController 只是旁听，键照样走 UIKit。NSH 的快捷键、滚动视图、按钮可能也对同一个键有反应（比如空格让滚动视图翻页），iOS 上没法拦。`captureKeys` 在 iOS 上不起作用。
- 手柄的 Options / Share 键：系统可能先拿去做手势（截屏、录屏），游戏收到会晚一点或者收不到。
- 系统自动重复、多人分手柄、震动、手柄灯、键位重映射。
- Android。

## 验收

- `nge-platform`（在 Mac 上跑）：
  - HID 表：字母、数字（0 在 9 后面）、空格、方向键、修饰键；键名不重复。
  - 和 Mac 的键名表是同一套名字。
  - 和 GameController 的 `GCKeyCode` 常量对得上（14 个，覆盖每一段）。
  - `Keyboard`：按住不重复、先松开后按下、全部松开；按着 ⌘ 时按下的键不发，⌘ 松开以后也不发、重新按下才发；修饰键照发。
- `cargo check --workspace --target aarch64-apple-ios` 通过。
- 还没手测：
  - iPad 连键盘、iPhone 连手柄玩平台跳跃，数字键选关。
  - 按着方向键进后台再回来、拉下控制中心再收起，角色没有一直走。
  - 打字时：NSH 输入框里打字游戏是否也收到（预期收到，见「不做」第一条）；空格、方向键会不会同时让页面滚动。
  - ⌘ + 字母不触发游戏。

## 修过的问题

- 只听了进后台：控制中心、来电横幅、分屏切走焦点时 App 只是失去焦点，按着的键会卡住。加了 `willResignActive` / `didBecomeActive`，失去焦点时补发松开、不读输入。
- 只看游戏视图自己的 `isHidden`：放在被隐藏的容器里时照样读。改成沿父视图一路往上查。
- iOS 上 HID 表的 `code`（只有测试用）报未使用：HID 表只在 iOS 和测试里编译，`code` 只在测试里有。
- 文档原先说 iOS 上「键不会被吞掉，不用管」：其实是拦不住，键会同时到 NSH 的控件那里。
