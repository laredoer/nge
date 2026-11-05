# 触屏按钮

> 前置：键盘（[keyboard.md](keyboard.md)）、TSX 绑定（[m3-tsx.md](m3-tsx.md)）。

## 目标

平台跳跃在 iPhone 上能玩。键盘和手柄只在 Mac 上转发，iOS 只转发触摸，游戏又不读触摸，所以真机上没法操作。

**验收标准**：iPhone 上平台跳跃屏幕下方有 ←、→、跳、重来四个按钮，按住 → 的同时按跳能边跑边跳；Mac 上不显示这些按钮。

## 做法

按钮按下、松开时往引擎发 `key` 命令（`{ code, down, repeat: false }`），和 Mac 上键盘、手柄按钮走同一条路进 `Input`。Rust 的角色控制不用改，按钮用游戏已经认的键名：← `ArrowLeft`、→ `ArrowRight`、跳 `Space`、重来 `KeyR`。

### 引擎（`ts/nge.tsx`）

- `key(game, code, down)`：发一次 `key` 命令。
- `heldKey(send)`：一个按钮上按着的手指。第一根手指按下时 `send(true)`，最后一根松开（或被系统取消）时 `send(false)`；没按着时的松开不发。
- `<KeyButton game code label class>`：用 `onTouchStart` / `onTouchEnd` / `onTouchCancel` 接手指，交给 `heldKey`。不用 `onPressDown` / `onPressUp`：同时按住两个按钮（跑着跳）要靠各自的触摸。

### 平台跳跃

- 只在手机、平板上显示（`platform.desktop` 是 `false`），叠在游戏视图下方：左边 ← →，右边跳、重来。
- 手机上 HUD 的操作说明换成短的，不写键盘按键。

## 不做

- 摇杆、滑动手势、按钮布局可配置。
- iOS 的硬件键盘和手柄（见 keyboard.md、gamepad.md 的「不做」）。

## 验收记录

- `nsh test`（`examples/platformer/App.test.tsx`）：手机上有四个按钮、Mac 上没有，`heldKey` 的多根手指、没按着时松开、重复按下。变异检查：去掉「没按着时松开不发」、去掉 Mac 上隐藏按钮，各有一个测试会失败。
- 真机（iPhone 16 Plus）手测能玩。
