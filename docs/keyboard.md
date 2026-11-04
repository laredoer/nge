# 键盘输入（Mac）

> 前置：M2（NSH 绑定）、M3（TSX）。

## 目标

Mac 上能用键盘玩：Rust 游戏每帧查哪些键按住了（方向键 / WASD 移动），TSX 收到按下 / 松开事件。

**验收标准**：
- `examples/dodge`：← → 或 A / D 控制飞船左右移动，和触摸 / 鼠标一起用都不打架；
- `examples/scene`：方向键让飞船补间移动一格，按住时连续移动；
- 在 TSX 的输入框里打字时，游戏收不到这些键；切到别的窗口时，按住的键全部算松开。

## 为什么不用 nsh 的 keymap

nsh 0.3 的 `nsh/keymap` 只报快捷键「按下了」（`KeyCommand`），没有松开，做不了「按住移动」。所以平台层自己在 AppKit 里装一个本地事件监听（`NSEvent addLocalMonitorForEventsMatchingMask:handler:`），拿到 keyDown / keyUp / flagsChanged。

## 键名

用 Web `KeyboardEvent.code` 的名字，按键盘上的位置命名，和输入法、键盘布局无关：`KeyA` … `KeyZ`、`Digit0` … `Digit9`、`ArrowLeft` / `ArrowRight` / `ArrowUp` / `ArrowDown`、`Space`、`Enter`、`Escape`、`Tab`、`Backspace`、`Delete`、`ShiftLeft` / `ShiftRight`、`ControlLeft` / `ControlRight`、`AltLeft` / `AltRight`、`MetaLeft` / `MetaRight`、`Minus`、`Equal`、`BracketLeft`、`BracketRight`、`Backslash`、`Semicolon`、`Quote`、`Backquote`、`Comma`、`Period`、`Slash`、`F1` … `F12`、`Home`、`End`、`PageUp`、`PageDown`。小键盘、CapsLock（它是锁定状态，物理松开没有事件）等别的键不转发。

## 数据流

平台层（主线程）收到按键 → 同时
- 发 command `key` 给引擎：`{ code, down, repeat }`，引擎把它记进 `Input`；
- 发事件 `key` 给 NSH：同样的 JSON，TSX 里 `onKey`。

### Rust：`Input`

- `key_down(code)`：现在按着；`key_pressed(code)` / `key_released(code)`：这一帧刚按下 / 刚松开（同一帧里按下又松开时两个都是 true）。
- `keys_down()`：按着的所有键。
- `axis(negative, positive)`：两组键各有一个按着时返回 -1 / 1，都按或都没按是 0。
- 自动重复（按住不放时系统重复发的 keyDown）不改状态。

### TSX：`onKey`

`gameView()` 的内置事件 `key: { code, down, repeat }`。

## 什么时候转发

- 只转发给所在窗口是 key window 的游戏视图。
- 第一响应者是文本编辑（NSText，比如 TSX 的输入框）时不转发，按键照常给输入框。
- 按着 ⌘ 的按下不转发、照常往下传（⌘Q、菜单快捷键照常用）；Ctrl / Option 组合照样转发。
- 视图被隐藏（自己或祖先 `hidden`）时不转发。
- 转发了的 keyDown 里，游戏要的键不再往下传（NSH 不处理时，比如没东西可滚的方向键，AppKit 会「咚」一声），别的照常往下传给 NSH。游戏要哪些键由游戏视图的 prop `captureKeys`（键名数组）决定，不填时是字母、数字、方向键、Space、Enter；所以默认情况下，游戏视图在屏幕上时，NSH 里用这些键的行为（方向键滚动、Space / Enter 激活聚焦的按钮、不带修饰键的 keymap 快捷键）收不到。Tab、Esc、Home / End 等默认留给 NSH，焦点切换和返回不受影响。keyUp 和 flagsChanged 总是往下传。
- 游戏用到默认名单以外的键（比如用 Esc 暂停）时，要自己写进 `captureKeys`，不然 NSH 不处理这个键时每次都会「咚」一声。
- `captureKeys` 是 macOS 平台层自己的 prop，不转给游戏，游戏不要注册同名的 prop；iOS 上它只是一个没人读的 prop。
- 窗口不再是 key window（切到别的 App、别的窗口）或游戏视图离开窗口时，平台层给每个按着的键补发一次松开。
- 同一个窗口里有几个游戏视图时，每个都收到按键；但字母 / 数字键被第一个视图吞掉以后，后面的视图收不到。一个窗口只放一个要收键盘的游戏视图。

## 实现上的决定

- macOS 的虚拟键码（`kVK_*`，按键盘位置）→ `code` 的表放在 `nge-platform`，纯函数，单元测试覆盖。
- 修饰键走 flagsChanged：看 `modifierFlags` 里区分左右的那一位（IOKit 的 `NX_DEVICE*KEYMASK`）判断按着还是松开，不按「集合里有没有」切换，所以错过一次事件也不会反过来。事件里没有区分左右的位（远程桌面、合成事件）时看这一族的 Shift / Control / Option / Command 位（键名照样分左右，来自 keyCode）。
- 平台层自己记一份按着的键，用来补发松开；引擎那边的 `Input` 只看收到的事件。松开只要按下时转发过就转发，不管当时焦点在哪。
- 「切走时补发松开」有三处：任何窗口 resign key 的通知、display link 的每帧回调、每个按键事件开头；视图离开窗口时也补发。
- 按着 ⌘ 按下的键不转发；松开 ⌘ 以后这个键的自动重复会当成一次新的按下转发（`repeat: false`），引擎和 TSX 的状态一致。
- 判断逻辑（`keys::decide`）是不依赖 AppKit 的纯函数，单元测试覆盖；`macos.rs` 只负责从 `NSEvent` / `NSWindow` 取出事实。

## 范围外

- iOS / iPad 的硬件键盘（`pressesBegan`）、Android
- 手柄
- 文本输入（游戏里打字）、键位重映射

## 验收记录（2026-11-04，Mac）

- **单元测试**：`nge-ecs` 新增 3 个（按下 / 松开 / 自动重复 / 同帧按下又松开 / 没按过的松开、`axis` 和 `keys_down`、`key` command 进 `Input`）；`nge-core` 1 个（`key` 的 JSON 格式、保留名）；`nge-platform` 新增 9 个（键码表：常用键、字母 / 数字 / F 键齐全且不重复、CapsLock 不转发；`decide`：按下 / 松开 / 自动重复和吞键名单、不可收或按着 ⌘ 时放行但松开照样转发、没转发过按下的自动重复当成新按下、修饰键看标志位不会反过来；默认吞键名单；修饰键标志位的兜底）；`examples/dodge` 1 个（← 按住移动、A 按住一直到边上停住）。
- **修过的问题**：修了修饰键反相卡住、CapsLock、吞键影响 NSH 的焦点 / 激活 / 滚动、切走不补发、隐藏视图收键、方向键「咚」、远程桌面下修饰键不转发。
- **TS**：`nsh check` 通过 `examples/scene`、`examples/physics`、`examples/scene3d`。
- **iOS**：`cargo check --workspace --target aarch64-apple-ios` 通过（键盘只在 macOS 上）。
- **实跑**：`dodge` 和 `scene`（`nsh dev --server-only`）在 Mac 上启动，dev server 显示 macOS host 已连接。按键手感（按住移动、⌘Tab 切走后不卡键、会不会「咚」）还没手测，macOS 事件监听这层没有自动化测试。
