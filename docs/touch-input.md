# 触屏控制：虚拟摇杆、手势、按钮补齐

> 前置：触屏按钮（[touch-controls.md](touch-controls.md)）、输入映射（[input-actions.md](input-actions.md)）、Android（[android.md](android.md)）、Web（[web.md](web.md)）、编辑器（[editor.md](editor.md)）。

## 目标

手机上现在只有按住发动作的 `<ActionButton>`：走路只能左右两个键，长坂坡没有触屏操作，手机和手机浏览器上玩不了；游戏也没有现成的手势（拖动、捏合）可用。

- **虚拟摇杆** `<VirtualStick>`：浮动（拇指按下的地方就是中心）或固定位置；死区、最大半径可配；输出二维值直接驱动一个动作（走 `action` command，和 `<ActionButton>` 同一条路，不发键名）；可以量化成八方向、四方向；每根手指独立，左手摇杆、右手按钮同时按。
- **手势**：轻点、长按、滑动（方向和速度）、拖动、双指捏合缩放、双指旋转。识别是纯函数状态机：输入触摸点序列，输出手势事件。游戏用它做拖动平移、捏合缩放、在画面上点选（再用 `screenToWorld` 换成世界坐标）。
- **按钮补齐**：`<ActionButton>` 加按住连发、冷却显示（都可选），按下时有视觉反馈。

**验收标准**：
- 长坂坡在手机和手机浏览器上能玩：左下摇杆移动，右下攻击（按住连发）、突刺（显示冷却）、跳；只在触屏设备上显示；结束画面点一下重开。
- 平台跳跃手机上的 ← → 换成虚拟摇杆（推多少走多快），游戏画面上双指捏合缩放镜头。
- 编辑器：Mac 触控板捏合（或 Control + 滚轮）缩放画布；触屏上双指捏合缩放、双指平移。
- 手势、摇杆的纯逻辑有单元测试；已有的示例和测试不回归。

## nsh 0.4 的触摸模型

查过 nsh 0.4 的类型和三端平台层，结论：

- TSX 拿到的是 React Native 式的原始触摸：`onTouchStart` / `onTouchMove` / `onTouchEnd` / `onTouchCancel`，事件里 `changedTouches` 是这次变了的手指，`touches` 是所有按着的手指。另有 responder 系统（`onResponder…`）和高层的 `onPress` / `onLongPress` / `onPanStart` / `onPanEnd` / `onPinchStart` / `onPinchEnd` / `onRotationStart` / `onRotationEnd`；后几个只在开始和结束时到 JS，中间的值只驱动原生的动画值（`pinch={{ scale }}`），JS 只能 `getValue()` 去读。所以手势自己从原始触摸算。
- 一根手指一个 `identifier`，按着期间不变；不同手指的触摸各自交给按下时所在的视图（`ActionButton` 已经靠这个在 iPhone 上边跑边跳）。
- 坐标：`locationX` / `locationY` 是相对**按下时的那个视图**（`target`）的 point，`pageX` / `pageY` 相对整个界面；`timestamp` 是毫秒，只能拿来算差。
- 三端来源：

| 平台 | 来源 | 手指 id | 时间戳 |
|---|---|---|---|
| iOS | UIKit 的 `UITouch`（`nsh-platform-ios` 的 `touch.rs`） | 按着期间固定 | `UITouch.timestamp × 1000`（开机以来） |
| Android | `MotionEvent`（运行时的 `TouchTracker.java`，按 `getPointerId` 跟踪） | 运行时自己递增编号，不复用 | `getEventTime`（开机以来） |
| Web | Pointer Events（`nsh.js`），根元素 `touch-action: none` | `pointerId` | `performance.now()` |
| macOS | 鼠标映射成一根手指；触控板的捏合、旋转（`magnifyWithEvent:` / `rotateWithEvent:`）和 Control + 滚轮只驱动 `pinch` / `rotation` 值，开始和结束到 JS | 鼠标一个 id | — |

  三端交到 JS 的形状一样，所以识别逻辑只写一份（TSX），只用 id、`locationX/Y`、时间戳的差，不碰平台细节。
- Mac 的触控板双指滑动是滚动（wheel），nsh 只交给滚动视图，普通视图收不到；鼠标只有一根「手指」。所以 Mac 上能做的只有捏合（和旋转），双指平移做不了（见「不做」）。
- 游戏视图上的触摸 `gameView()` 已经同时转成引擎的 `touch` command（Rust 的 `Input::pointers`，带世界坐标），用户的 `onTouch…` 照常调用。

## 做法

### 虚拟摇杆（`ts/stick.ts`、`ts/touch.tsx`）

纯函数（都在 `ts/stick.ts`）：

- `stickValue(dx, dy, { radius, deadZone })`：手指相对中心的偏移（屏幕坐标，y 朝下）→ 摇杆值（y 朝上）。长度在 `deadZone × radius` 以内是 `[0, 0]`；外面按 `(长度 − 死区) / (半径 − 死区)` 重新铺满 0 … 1，超过半径的按 1（方向不变）。死区外一点点就是一点点，不会从 0 跳到死区的值。
- `snapDirection(v, 8 | 4)`：八方向按 45° 一格取最近的方向，四方向取绝对值大的那个轴；各分量是 −1 / 0 / 1（引擎的二维轴会把 `[1, 1]` 限到长度 1）。`[0, 0]` 还是 `[0, 0]`。
- `knobOffset(dx, dy, radius)`：摇杆头画在哪（偏移限在半径以内）。
- `stickStep(state, touch, { mode, center })`：跟踪一根手指的状态机。没手指时第一根按下的接管（浮动模式中心是按下的点，固定模式是 `center`）；别的手指不管；接管的那根移动更新位置，抬起或被取消（cancel）放开；同一个 id 又按下（丢了 end）当新的一次。
- `stickCommand(prev, next, …)`：要不要发、发什么。摇杆值变了才发；值是 `[0, 0]`（死区里、抬起）发松开，不然发按下带 `x`、`y`。值保留两位小数，免得手指抖一下就发一条。

组件 `<VirtualStick game action player? mode="float"|"fixed" radius=60 deadZone=0.2 directions="analog"|"8"|"4" center? source="stick" class style>`：

- 自己就是一块收触摸的区域（调用方用 `class` / `style` 摆位置和大小，比如屏幕左下半边）。底座和摇杆头是 `pointerEvents="none"` 的子视图，这样手指的 `target` 总是区域本身，`locationX/Y` 就是区域里的坐标。
- 每次触摸变化走一遍 `stickStep`，算出的值交给 `stickCommand`，要发时用 `act()`（`action` command，`source` 默认 `"stick"`）。动作是一维轴时引擎只取 `x`。
- 没按着时，浮动模式把底座淡淡地画在 `center`（不填是区域左下角往里一个半径），固定模式就在 `center`。
- 组件卸载时还按着就发一次松开。

### 手势（`ts/gestures.ts`）

`gestureStep(state, input, options)` 返回 `{ state, events }`，状态是不可变的普通对象。输入是触摸点（`{ phase: "start" | "move" | "end" | "cancel", id, x, y, t }`，`t` 是事件的毫秒时间戳），外加长按的计时到了（`{ phase: "hold", id }`，由计时器喂进来：各平台时间戳的起点不一样，JS 拿不到同一个钟，所以计时放在外面，状态机只认「到点了」）。

| 事件 | 什么时候 | 带什么 |
|---|---|---|
| `tap` | 一根手指按下、没挪出 `slop`（10 pt）、在 `longPress`（500 ms）之前抬起 | `x, y` |
| `longPress` | 一根手指按住没挪出 `slop`，计时到了；计时器晚了、抬起时已经超时也补发 | `x, y` |
| `dragStart` / `drag` / `dragEnd` | 一根手指挪出 `slop` 以后；长按以后再挪也是拖动 | `x, y`；`dx, dy`（这一步挪了多少）；`tx, ty`（从按下的地方算）；`dragEnd` 加 `vx, vy`（最后 100 ms 的速度，pt/s）和 `cancelled` |
| `swipe` | 拖动正常抬起（不是取消），总位移 ≥ `swipeDistance`（30 pt）、抬起时速度 ≥ `swipeSpeed`（300 pt/s） | `direction`（`left` / `right` / `up` / `down`，按位移大的轴，屏幕的上是 `up`）、`vx, vy`、`speed` |
| `pinchStart` / `pinch` / `pinchEnd` | 第二根手指按下 | 两指中点 `x, y`；`scale`（现在的距离 / 开始的距离）、`rotation`（度，屏幕上顺时针为正，和 nsh 的 `onRotation…` 一样）；`pinch` 加这一步的 `factor`（缩放倍数）、`turn`（转了多少度）、`dx, dy`（中点挪了多少）；`pinchEnd` 加 `cancelled` |

- 多指：只认前两根。一根在拖动时第二根按下，拖动以 `cancelled: true` 结束、开始捏合；两根中任一根抬起，捏合结束，剩下的手指不再算任何手势，直到全部抬起。第三根以后的手指不管。
- 取消（系统拿走触摸、来电）：正在进行的拖动 / 捏合以 `cancelled: true` 结束，不发 `tap` / `swipe`，这一轮剩下的手指都不算，直到全部抬起。
- 不认识的 id 的 move / end（按在别的视图上的手指出现在 `changedTouches` 里）不管。
- 两指一开始距离是 0（叠在一起）时，`scale` 按 1 算，不除零。
- 旋转角度跨过 ±180° 时按最短的方向算增量。

接到视图上：`gestureHandlers(onGesture, options?)` 返回 `onTouchStart` / `onTouchMove` / `onTouchEnd` / `onTouchCancel`，把 `changedTouches` 的每根手指换成输入喂给状态机，再把事件交给 `onGesture`；按下时开一个 `longPress` 毫秒的 `setTimeout` 喂 `hold`。坐标用 `locationX/Y`，所以要放在子视图不收触摸的视图上（游戏视图正好）。点选世界坐标：`screenToWorld(camera, size, [e.x, e.y])`（TSX 自己管相机时，比如编辑器、策略游戏；相机跟着角色走时 TSX 不知道相机在哪，用引擎的 `tap` 事件或 Rust 的 `Pointer.world`）。

### 按钮补齐（`<ActionButton>`）

- `repeat`（秒）：按住时每隔这么久再按一次（先等 `repeatDelay`，默认 0.4 秒）。每次重按发一对松开、按下，通常落在同一帧里。
  - 引擎原来对直接驱动的动作，一帧里「松开又按下」不算刚按下（键盘的键算）。改成和键一样：上一帧按着、这一帧松开过又按下，`pressed`、`released` 都是 true；发给 TSX 的 `action` 事件也补一对松开、按下（键盘的键一帧里松开又按下时也一样补发，原来 TSX 收不到这次按下）。
  - 计时是 `repeater(send, { delay, interval }, timers)`：按下立刻发按下，然后定时重按，松开时清掉计时器；计时器可以换（测试用假的）。
- `cooldown`：一个返回 0 … 1 的函数（游戏给，1 是刚用过，0 是好了），按钮上盖一层从下往上退的暗色（`cooldownFill`，超出范围、不是数的截断成 0 … 1）。冷却中照样能按，能不能出招由游戏决定。
- 按下时的反馈：按着时背景变亮、按钮缩小一点（`pressedLook`），由手指的按下 / 松开驱动，不靠 `active:`（那是 press 的状态，这里收的是原始触摸）。
- 原来的 `ActionButton`、`heldKey`、`actionButtonId`、`act` 挪到 `ts/touch.tsx`，`nge.tsx` 原样导出，用法不变。

### 示例

- **长坂坡**：
  - 场景加一张只给触屏用的绑定表（`actions` prop）：`move`（二维轴）、`attack`、`thrust`、`jump`（按钮），都不绑键（键盘和手柄照旧走 `onKey`，不改）。
  - 触屏（`platform.desktop` 是 `false`）时：左半边是浮动摇杆（八方向，驱动 `move`），右下攻击（按住每 0.18 秒连发一次，连段靠这个）、突刺（冷却显示 = 剩余冷却 / 1.5 秒）、跳。
  - `onAction` 收到的动作和键盘的输入合在一起（`touchInput`，纯函数）：`move` 的方向当成方向键，`attack` / `thrust` / `jump` 按下的那一下算一次。
  - 结束画面上「按 R 重新开始」在触屏上换成「点一下重新开始」，点结束画面重开。说明文字只在桌面上显示（原来就是）。
- **平台跳跃**：手机上的 ← → 换成摇杆（左下，模拟量：推多少走多快），跳、重来还是按钮；游戏画面上双指捏合缩放镜头（0.6 – 1.6 倍，`zoomBy` 纯函数），只在触屏上。
- **编辑器**：
  - 画布外面的视图收 nsh 的捏合（Mac 触控板、Control + 滚轮）：开始时记下视图，捏的时候每 33 ms 读一次 `pinch` 的值，以鼠标最后在画布上的位置为中心缩放（`zoomAt`，鼠标没进过画布时用中心），结束时用最后的倍数。
  - 触屏：画布的触摸也交给手势；第二根手指按下时取消正在进行的工具操作（框选、拖动），然后双指捏合缩放、中点移动就平移（`pinchView`：先以中点缩放再平移）。

## 不做

- Mac 触控板的双指平移：滚动只交给滚动视图，游戏视图收不到；编辑器照旧用平移工具（H）、方向键。
- 双击、三指以上的手势、边缘滑动（系统手势会先拿走）。
- Rust 一侧的手势识别：Rust 游戏读 `Input::pointers()` 自己算；手势只在 TSX。
- 摇杆的模拟量给 TSX：`action` 事件里轴还是 −1 / 0 / 1（Rust 的 `Actions::vec` 是连续的）。长坂坡的逻辑在 TSX，所以轻推（不到一半）不走。
- 按钮布局、摇杆位置运行时可调；按住连发的加速。
- 触屏上的暂停按钮（长坂坡）。
- 手机自己的震动反馈。

## 验收

- 全量检查通过：`cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、平台跳跃 / 长坂坡 / 编辑器的每个 `nsh test` 和 `nsh check`、`cargo check --target aarch64-apple-ios -p platformer -p sanguo`、`tools/check-wasm.sh`、`tools/check-android.sh`。
- 单元测试（都在 Mac 上跑）：
  - `nge-ecs` `actions`：`a_driven_button_released_and_pressed_again_within_one_frame_is_a_new_press`（直接驱动的按钮一帧里松开又按下算重按，TSX 收到一对松开、按下；键也补发）。
  - 平台跳跃 `touch.test.tsx`（`ts/stick.ts`、`ts/gestures.ts`、`ts/touch.tsx` 的纯逻辑）：死区和重新铺满、半径钳制、y 朝上、八 / 四方向量化、摇杆头限位；浮动 / 固定中心、多指时只认第一根、抬起和取消放开、丢了 end 的 id；发什么（去重、两位小数、0 是松开）；轻点、长按（计时器、晚了补发）、拖动（每步和总位移、速度）、滑动（距离、速度、方向、取消不算）、捏合（scale、factor、中点位移）、旋转（顺时针为正、跨 ±180°）、拖动中加第二指、第三指、捏合被取消、叠在一起不除零、不认识的手指；`gestureHandlers` 的长按计时开关；按住连发、冷却截断、按下的样子。
  - 平台跳跃（Rust）`tsx_uses_action_buttons_and_groups_the_players`：`game.tsx` 里走路是驱动 `move` 的 `<VirtualStick>`，跳、重来是 `<ActionButton>`。
  - 平台跳跃 `App.test.tsx`：手机上有摇杆和跳、重来，没有 ← →；`hud.test.tsx`：摇杆区域让开安全区、镜头缩放的范围和坏的倍数。
  - 长坂坡 `touch.test.tsx`：触屏的表不绑键、摇杆方向和松开、按钮按下才算（连发每一下、暂停不算、只认第一个玩家）、和键盘合并、突刺冷却比例、触屏上有摇杆和三个按钮而 Mac 上没有。
  - 编辑器 `canvas.test.tsx`：`pinchView` 以中点缩放、跟着中点平移。
- 下面这些都做过变异：把对应的逻辑改坏，测试会失败，再改回来（41 个；第一次有 1 个没抓到：「摇杆松开不归零」，引擎发松开时方向本来就是 0，补了一条带着方向的松开的测试）。

| 测试 | 改坏（变异） |
| --- | --- |
| `touch.test.tsx › 摇杆：死区里是 0…` | 死区外不重新铺满 |
| `touch.test.tsx › 摇杆：超过半径按 1…` | 超过半径不钳到 1 |
| `touch.test.tsx › 摇杆：死区里是 0…` | 摇杆 y 不翻成朝上 |
| `touch.test.tsx › 摇杆：八方向…` | 八方向按 90° 一格 |
| `touch.test.tsx › 摇杆：八方向…` | 四方向取小的轴 |
| `touch.test.tsx › 摇杆头限在半径以内` | 摇杆头不限在半径里 |
| `touch.test.tsx › 浮动摇杆：第一根按下的手指接管…` | 别的手指抢走摇杆 |
| `touch.test.tsx › 手指抬起、被取消都放开…` | 取消（cancel）不放开 |
| `touch.test.tsx › 固定摇杆：中心是给定的位置` | 固定模式也以按下的点为中心 |
| `touch.test.tsx › 摇杆发什么…` | 值没变也发 |
| `touch.test.tsx › 摇杆发什么…` | 回到 0 不发松开 |
| `touch.test.tsx › 轻点…` | 不看 slop，一挪就是拖动 |
| `touch.test.tsx › 长按…` | 计时器晚了不补发长按 |
| `touch.test.tsx › 长按…` | 拖动中计时到了也算长按 |
| `touch.test.tsx › 滑动…` | 滑动不看速度 |
| `touch.test.tsx › 滑动…` | 取消的拖动也算滑动 |
| `touch.test.tsx › 拖动中第二根手指按下…` | 第二根手指按下时拖动不结束 |
| `touch.test.tsx › 旋转…` | 旋转跨 ±180° 不绕回 |
| `touch.test.tsx › 捏合…` | factor 按开始的距离算 |
| `touch.test.tsx › 两指叠在一起开始不除零…` | 两指叠在一起时除零 |
| `touch.test.tsx › 捏合…` | 捏合结束后剩下的手指还算轻点 |
| `touch.test.tsx › 拖动中第二根手指按下…` | 第三根手指顶替捏合的手指 |
| `touch.test.tsx › gestureHandlers…` | 抬起时不关长按计时 |
| `touch.test.tsx › 两指叠在一起开始不除零…` | 丢了 end 的 id 再按下不当新的 |
| `touch.test.tsx › 按住连发…` | 连发不先松开 |
| `touch.test.tsx › 按住连发…` | 松开时不停连发 |
| `touch.test.tsx › 冷却…` | 冷却不截断 |
| `touch.test.tsx › 冷却…` | 按下时没有反馈 |
| `a_driven_button_released_and_pressed_again_within_one_frame_is_a_new_press` | 直接驱动一帧里松开又按下不算重按 |
| `a_driven_button_released_and_pressed_again_within_one_frame_is_a_new_press` | TSX 收不到重按前的松开 |
| `hud.test.tsx › 双指捏合缩放镜头…` | 镜头缩放不限范围 |
| `hud.test.tsx › 双指捏合缩放镜头…` | 坏的倍数也乘 |
| `hud.test.tsx › 摇杆的区域在左下，让开安全区` | 摇杆区域不让开安全区 |
| `App.test.tsx › 手机上有摇杆和触屏按钮…` | 平台跳跃的摇杆不驱动 move |
| `sanguo touch.test.tsx › 摇杆的方向当成方向键…` | 长坂坡收别的玩家的动作 |
| `sanguo touch.test.tsx › 攻击、突刺、跳…` | 暂停时触屏按钮也算 |
| `sanguo touch.test.tsx › 和键盘合在一起…` | 触屏的方向盖过键盘 |
| `sanguo touch.test.tsx › 摇杆的方向当成方向键…` | 摇杆松开不归零 |
| `sanguo touch.test.tsx › 触屏设备上有摇杆和三个按钮，Mac 上没有` | Mac 上也显示触屏操作 |
| `sanguo touch.test.tsx › 绑定表只有触屏驱动的动作…` | 触屏的表绑了键 |
| `canvas.test.tsx › 双指捏合…` | 捏合以挪动后的中点缩放 |

## 还没手测

这一项没有起模拟器，也没有开窗口跑，下面这些只有编译检查和纯函数的单元测试：

- iPhone、Android、手机浏览器上摇杆和按钮同时按（左手摇杆、右手按钮）、手感（死区 0.2、半径 60 合不合适）。
- 长坂坡、平台跳跃在手机上的整局。
- 平台跳跃的双指捏合缩放镜头。
- 编辑器的触控板捏合：nsh 的命中测试会不会把捏合交给游戏视图外面那层（游戏视图是原生组件）；33 ms 读一次 `pinch` 值够不够顺。
- 编辑器在触屏上的双指平移、缩放（编辑器是桌面应用，没有触屏设备试）。
- 浏览器嵌在别的页面里（`nsh-embedded`，`touch-action: pan-x pan-y`）时，双指会不会被页面拿去滚动、缩放。
