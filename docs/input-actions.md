# 输入映射：动作、改键、多人分手柄、扳机、震动

> 前置：键盘（[keyboard.md](keyboard.md)）、手柄（[gamepad.md](gamepad.md)）、iOS 的手柄和键盘（[ios-input.md](ios-input.md)）、触屏按钮（[touch-controls.md](touch-controls.md)）、Android（[android.md](android.md)）、Web（[web.md](web.md)）、存档槽（[saves.md](saves.md)）。

## 目标

现在游戏直接读键名（`Space`、`GamepadA`）。改键、两个人各用一个手柄都做不了，扳机只有「按过一半」，手柄不会震。加一层「动作」：

- 游戏声明动作（跳、移动、攻击），每个动作绑若干来源：键盘键、手柄按钮、摇杆和十字键的方向、扳机、摇杆的模拟量。游戏只问动作：按着、刚按下、刚松开、轴的值。Rust 和 TSX 都能问。
- 运行时改绑定：按任意键来绑、查冲突、恢复默认、存在设备上。
- 手柄按设备分：每个玩家一份绑定、一个手柄，动作按玩家问；键盘可以拆成两套给两个人；手柄接入、拔出有事件。
- 扳机给 0 … 1 的值，`GamepadL2` / `GamepadR2` 的按钮语义不变。
- 手柄震动：强弱两个马达加时长。
- 原来直接读键名的接口（`Input::key_down`、`axis`、`stick`，TSX 的 `onKey`）不变，已有示例不用改。

**验收标准**：
- 平台跳跃的跑、跳、重来、换存档、换语言、联网开关都问动作，不直接读键。
- 平台跳跃有改键面板：点跳跃的某个键，再按任意键换掉它；和别的动作冲突时先问；能恢复默认；重开后还在。
- 接上第二个手柄时出现第二个角色，由第二个手柄控制；拔掉就消失。
- 角色落地、被撞回起点时手柄震一下。
- 已有的示例和测试不回归。

## 做法

### 数据流

原来的路不动，旁边加一条按设备的：

```
平台层 ──key（合并后的键名）────────────→ Input（按着的键）          ← 原来的
       ──gamepad（合并后的摇杆、扳机）──→ Input（摇杆、扳机）        ← 原来的，多了扳机
       ──pad（一个手柄的整份状态）──────→ Input（每个手柄各一份）    ← 新的
TSX    ──prop actions（绑定表）─────────→ Actions（每帧算出动作的状态）
Actions ──action（动作变了）、pad（手柄接入 / 拔出、归谁）──→ TSX
游戏   ──Rumble / TSX 的 rumble command──→ nge.rumble ──→ 平台层（不到 TSX）
```

### 按设备的手柄（平台层 → 引擎）

- 平台层给每个手柄一个号（`pad::Ids`：同一个手柄一直是同一个号，新来的用最小的空号；Android 用系统的设备号，浏览器用 `Gamepad.index`）。
- 每个手柄的状态（按着的键名、去掉死区的两根摇杆、两个扳机 0 … 1）变了时发内置 command `pad { id, connected, keys, left, right, l2, r2 }`（`pad::Devices`）。第一次发就是接入；读不到了发 `connected: false`。发的是整份状态，丢一条也不会卡键。
- 窗口失焦、进后台这些「全部松开」的时候，每个手柄发一份空状态，不算拔出（玩家不会掉）。
- 合并的那条路照旧：任何一个手柄按着就发 `key`，摇杆取推得最远的发 `gamepad`。`gamepad` 多了 `l2`、`r2`（各取最大的）。
- 引擎的 `Input` 多了：
  - `pad(id)`：这个手柄的 `key_down` / `key_pressed` / `key_released`、`stick`、`trigger`。
  - `pads()`：连着的手柄号；`pads_connected()` / `pads_disconnected()`：这一帧接入、拔出的。
  - `trigger(Trigger::L2 | R2)`：合并后的扳机。
- 摇杆的死区还是长度 < 0.15 当 0；扳机限制在 0 … 1，坏的值当 0。
- 浏览器原来没有手柄，这次加上：每帧读 `navigator.getGamepads()`，只认 `mapping` 是 `"standard"` 的，按钮和轴按标准布局换成同一份读数（`web_pad.rs`）；页面隐藏、窗口没焦点时全部松开。

### 动作（`nge-ecs` 的 `actions.rs`）

绑定表是一份 JSON（TSX 的 prop `actions`，Rust 里是 `ActionMap`），每个玩家一张表，动作名对绑定：

```ts
{ players: [{
    jump: { kind: "button", keys: ["Space", "GamepadA"] },
    move: { kind: "axis", negative: ["ArrowLeft", "GamepadLeft"], positive: ["ArrowRight", "GamepadRight"], analog: ["LeftStickX"] },
    aim:  { kind: "axis2d", left: [], right: [], down: [], up: [], analog: ["RightStick"] },
  }] }
```

- 键名就是原来那套（键盘的 `KeyboardEvent.code`，手柄的 `Gamepad…`，摇杆方向和扳机也是键名）。
- `analog`：一维的轴可以是 `LeftStickX` / `LeftStickY` / `RightStickX` / `RightStickY` / `L2` / `R2`，二维的是 `LeftStick` / `RightStick`。
- `Actions` 资源每帧（`PreUpdate`）从 `Input` 算一遍：
  - `down(name)`、`pressed(name)`、`released(name)`、`value(name)`（一维）、`vec(name)`（二维）；`player(n)` 问第 n 个玩家的，不带的是第一个玩家。
  - 按钮：绑的键有一个按着就是按着；同一帧按下又松开时 `pressed` 和 `released` 都是 true。绑了 `GamepadL2` / `GamepadR2` 的按钮，`value` 是扳机的模拟量。
  - 轴：键给 −1 / 0 / 1，模拟量给连续的值，取绝对值大的；二维的取长度大的。
  - 没声明的动作：没按、值是 0。
- 手柄归谁：
  - 只有一个玩家时所有手柄都是他的（读合并后的，和原来一样，连上就能玩）。
  - 两个以上玩家时每人一个手柄：新接入的给最靠前的、还没手柄的玩家；拔掉了位置空出来，别人的不动（`Actions::assign` 可以手动改）。没分到手柄的玩家，手柄的键都算没按。
  - 键盘是大家共用的：两个人的表里写不同的键，就是一个键盘两个人。
- TSX 不跑每帧逻辑，动作的状态由引擎算好发过去：
  - `action { action, player, down, x, y }`：按钮按下、松开，轴的方向变了（过一半才算，x、y 是 −1 / 0 / 1）时发。
  - `pad { id, connected, player }`：手柄接入、拔出、换了玩家；每次收到绑定表时把连着的再报一遍（JS 重载后也知道）。
- 绑定表的 `capturing: true`（正在等玩家按键来绑）时所有动作都算没按，免得绑空格时角色跳起来。

### 改键（`ts/input.ts` 的 `inputMap()`，从 `ts/nge.tsx` 导出）

```ts
const input = inputMap({ key: "platformer", defaults: [P1, P2] });
<GameView actions={input.prop()} onAction={input.handle} onPad={input.handlePad} onKey={(e) => input.handleKey(e) || ...} />
```

- `bindings()`：现在的绑定表（signal）；`prop()`：给游戏视图的 `actions`。
- `slots(player, action)`：这个动作的每个键是哪个位置（`{ player, action, part, index }`，`part` 是 `keys`、`negative`、`up` 这些）。
- `capture(slot)`：等下一个按下的键（`handleKey` 收到的，手柄的键、摇杆方向、扳机也是键）。
  - `Escape` 取消。
  - 没冲突：换上。
  - 有冲突（键盘的键查所有玩家，手柄的键只查同一个玩家；同一个动作里的不算）：不换，`pending()` 给出要绑的键和冲突的位置，界面问玩家；`confirm()` 把两边对调（冲突的位置换成这个位置原来的键），`cancel()` 放弃。
  - `capturing()`：正在等的位置。
- `bind(slot, code)` 直接换（返回冲突，不为空时没换），`conflicts(slot, code)` 只查，`unbind(slot)` 去掉一个键；位置的 `index` 排在最后一个后面就是加一个键。
- `reset()` 恢复默认。
- 存在 nsh 的 `Storage` 里（键 `nge:input:<key>`，`{ v: 1, players }`）：改了就存；读出来时按默认的表对一遍，默认里没有的动作丢掉，新加的动作用默认的，结构不对的整份不要。测试可以传 `store`。
- 动作的状态：`down(action, player)`、`value(action, player)`（`[x, y]`），来自 `action` 事件；刚按下、刚松开就是事件本身。`padOf(player)`：这个玩家的手柄号（没有是 `null`），来自 `pad` 事件。
- Rust 写的游戏不经过 TSX 时，自己改 `Actions` 里的 `ActionMap`（`set_map`）；要「按任意键」时用 `Input::keys_pressed()`。

### 震动

- Rust：`Rumble` 资源，`rumble.play(player, low, high, seconds)`；TSX：`rumble(game, { player, low, high, duration })`。`low` 是低频（重）马达、`high` 是高频（轻）马达，0 … 1；时长最多 5 秒。
- 引擎按玩家找到手柄号（只有一个玩家时是全部手柄），发 `nge.rumble { pad, low, high, duration }`。平台层在把事件交给 NSH 之前截下来，不到 TSX。
- macOS、iOS：`GCController.haptics` 建 `CHHapticEngine`（左手柄是低频、右手柄是高频；手柄不分左右时用默认的位置、取两个里大的），放一段持续的震动。引擎按手柄缓存，手柄拔掉时丢掉；出错了丢掉缓存下次重建。在主线程的每帧回调里放。
- Android：`NshView.vibratePad`（仓库里改过的运行时）找到 `InputDevice`，Android 12 起用 `VibratorManager`（两个马达各放各的，只有一个时取大的），之前用 `InputDevice.getVibrator()`。
- 浏览器：`gamepad.vibrationActuator.playEffect("dual-rumble", …)`，没有就不震。
- 强度、时长怎么换成各平台的参数是纯函数（`rumble.rs`）。

### 示例（平台跳跃）

- 默认绑定在 `controls.json`，Rust（`include_str!`）和 TSX（`import`）读同一份：TSX 的 prop 到之前（和 Rust 的测试里）也能玩。
- Rust 的角色控制问 `move`、`jump`、`restart`；TSX 的换存档、换语言、联网开关问 `slot`、`lang`、`net`。
- 改键面板（`remap.tsx`，按 K 或点右上角的「改键」按钮）：列出跳跃的键，点一个（或点「+」加一个）再按任意键；冲突时显示撞了哪个动作和「换过来 / 取消」；「恢复默认」；关掉面板就不等按键了。
- 两个玩家的表：第二个玩家用 F / H / T 和第二个手柄（第一个玩家已经用了 L、K、N）。第二个手柄接入时场景里多一个角色 `player2`，拔掉就去掉。它只能跑、跳、掉下去回起点：不吃金币、不算血、不通关，相机也不跟它。
- 第一个玩家落地（在空中超过 0.2 秒）轻震一下，被撞或掉下去回起点时重震一下。
- 平台跳跃的表里有两个玩家，所以手柄的键只认按设备的那条路；原来测试里用合并的 `key GamepadA` 模拟手柄的地方改成发 `pad`。键盘、触屏按钮照旧。

## 不做

- 手机自己的震动（没接手柄时震手机）：Android 要 `VIBRATE` 权限，nsh 生成的清单里没有；iPhone 要另开一个 `CHHapticEngine`。先只震手柄。
- 提前停掉震动、震动曲线、扳机上的马达（Xbox 的冲击扳机、DualSense 的自适应扳机）。
- 浏览器里 `mapping` 不是 `"standard"` 的手柄：按钮顺序没有标准，不认。
- Android 上手柄一接上就知道：没有每帧的轮询，第一次按键或推摇杆时才算接入。
- macOS 上窗口不在前台时接入、拔出的手柄：回到前台才知道。
- TSX 收模拟量：`action` 事件里轴只有 −1 / 0 / 1。
- 组合键（Shift + X）、长按 / 双击、按住自动重复、鼠标和触摸当动作的来源（触屏按钮还是发键名，照样进动作）。
- 第二个玩家的完整玩法（金币、血、通关、分屏）。
- 陀螺仪、触摸板、手柄灯。
- 改键面板里改移动、重来这些：库支持，面板只做了跳跃。

## 验收

- 全量检查通过：`cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、平台跳跃 / 长坂坡 / 编辑器的每个 `nsh test` 和 `nsh check`、`cargo check --target aarch64-apple-ios -p platformer -p sanguo`、`tools/check-wasm.sh`、`tools/check-android.sh`。
- 单元测试（都在 Mac 上跑）：
  - `nge-core` `bridge`：`pad`、`gamepad` 的扳机、`rumble`、`nge.rumble`、`action`、`pad` 事件的 JSON 格式和默认值；新通道是保留名。
  - `nge-ecs` `input`：每个手柄各自的键、摇杆、扳机和边沿；接入、拔出只有一帧；两份状态之间的点按留着两个边沿；合并的扳机限幅；这一帧按下的键。
  - `nge-ecs` `actions`：按钮（几个键、点按）、一维和二维的轴、扳机既是模拟量又是按钮、没声明的动作；一个玩家时所有手柄都是他的；两个玩家各一个手柄、拔掉让位、第三个补上、手动对调；一个键盘拆给两个人；发给 TSX 的 `action` / `pad` 事件；绑键时都算没按；换表、坏表、去掉的动作；震动找到玩家的手柄、限幅、封顶。
  - `nge-ecs` `app`：`pad`、`gamepad` 的扳机进 `Input`。
  - `nge-platform` `pad`：扳机限幅和合并、一个手柄的状态、`Devices`（新的、变了的、没了的，失焦不算拔出）、`Ids`。`rumble`：截下 `nge.rumble`、队列封顶、震哪几个、各平台的单位。`web_pad`：标准布局的按钮、扳机、十字键、摇杆，只认标准布局，震动的参数。`android_keys`：每个设备自己的状态。`gc`（只在 Mac 上）：建得出 `CHHapticPattern`，没有手柄时读数是空的。
  - 平台跳跃（Rust）：手柄玩（按设备的 `pad`）、摇杆推一点走得慢、绑定跟着 `actions` 变、第二个手柄控制第二个角色、第二个角色的键盘和掉下去回来、落地和受伤震第一个玩家的手柄。
  - 平台跳跃（`nsh test`）：`input.test.tsx`（位置、换键、冲突、按任意键、Escape、对调、恢复默认、存取、按默认的表对一遍、坏档、动作和手柄的事件）；`controls.test.tsx`（默认绑定不冲突、键名、面板的提示、面板打开 / 列出 / 等按键 / 关掉、存的绑定读回来、恢复默认写回去、手机上也有按钮）。
- 下面这些测试都做过变异：把对应的逻辑改坏，测试会失败，再改回来（一共 92 个，都被抓到；有一个第一次没抓到，补了测试）。

| 测试 | 改坏（变异） |
| --- | --- |
| `input_channels_are_reserved` | pad 不是保留的 command |
| `input_channels_are_reserved` | action 不是保留的事件 |
| `pad_and_rumble_wire_formats` | pad 的 connected 默认 false |
| `pad_and_rumble_wire_formats` | rumble 的 duration 默认 0 |
| `each_pad_has_its_own_keys_sticks_and_triggers` | 新手柄不记接入 |
| `merged_triggers_are_clamped` | 扳机不限幅 |
| `each_pad_has_its_own_keys_sticks_and_triggers` | 帧末不清手柄的刚按下 |
| `each_pad_has_its_own_keys_sticks_and_triggers` | 帧末不清接入 |
| `each_pad_has_its_own_keys_sticks_and_triggers` | 没连过的也算拔出 |
| `a_tap_between_two_pad_states_keeps_both_edges` | 手柄状态不比刚松开 |
| `a_button_is_down_while_any_of_its_keys_is` | 按钮刚按下不看原来按着没有 |
| `a_button_is_down_while_any_of_its_keys_is` | 按钮还按着也算刚松开 |
| `a_tap_within_one_frame_is_pressed_and_released` | 一帧内的点按不算刚松开 |
| `pads_can_be_assigned_by_hand` | 多人时手柄键读合并的 |
| `with_two_players_each_gets_a_pad` | 多人时摇杆读合并的 |
| `with_two_players_each_gets_a_pad` | 拔掉的手柄不让位置 |
| `pads_can_be_assigned_by_hand` | 新手柄不入座 |
| `a_trigger_is_analog_and_still_a_button` | 轴不取模拟量 |
| `an_axis_takes_the_larger_of_its_keys_and_the_stick` | 轴两边都按不抵消 |
| `a_trigger_is_analog_and_still_a_button` | 扳机当按钮时不给模拟量 |
| `a_2d_axis_has_both_components` | 二维轴不取摇杆 |
| `nothing_is_down_while_capturing_a_key` | 绑键时动作照常算 |
| `changes_are_sent_to_tsx` | 轴没过一半也发方向 |
| `changes_are_sent_to_tsx` | 点按不补发按下 |
| `a_new_map_takes_over_and_reports_the_pads_again` | 换表后不再报手柄 |
| `an_action_dropped_from_the_map_is_released` | 去掉的动作不告诉松开 |
| `with_one_player_every_pad_is_theirs` | 一个玩家时手柄也报没人的 |
| `pads_can_be_assigned_by_hand` | 手动分手柄不对调 |
| `rumble_goes_to_the_players_pad` | 一个玩家时只震一个手柄 |
| `rumble_goes_to_the_players_pad` | 没手柄的玩家也发震动 |
| `rumble_goes_to_the_players_pad` | 没有的玩家也震 |
| `rumble_drops_nothing_to_play_and_caps_the_duration` | 震动时长不封顶 |
| `rumble_drops_nothing_to_play_and_caps_the_duration` | 强度是 0 也发 |
| `a_new_map_takes_over_and_reports_the_pads_again` | 去掉 actions prop 清空绑定 |
| `a_trigger_is_analog_and_still_a_button` | gamepad 不记扳机 |
| `rumble_goes_to_the_players_pad` | rumble command 不进队列 |
| `devices_report_new_changed_and_gone_pads` | 没变也发 pad |
| `releasing_keeps_the_pads_connected` | 失焦当成拔出 |
| `devices_report_new_changed_and_gone_pads` | 读不到的手柄不报拔出 |
| `ids_stay_with_their_pad_and_reuse_the_smallest_free_one` | 手柄号不回收 |
| `ids_stay_with_their_pad_and_reuse_the_smallest_free_one` | 手柄号不认老手柄 |
| `triggers_are_analog_and_merged_by_the_deepest` | 扳机合并不取大的 |
| `triggers_are_analog_and_merged_by_the_deepest` | 平台层扳机不限幅 |
| `a_reading_becomes_the_state_of_one_pad` | 按设备的摇杆不去死区 |
| `diff_releases_then_presses_and_reports_moved_sticks` | 扳机变了不发 |
| `rumble_events_are_taken_out_and_the_rest_passed_on` | 震动也交给 NSH |
| `the_queue_does_not_grow_without_bound` | 震动队列不封顶 |
| `targets_are_one_pad_or_all_of_them` | 指定了手柄也全震 |
| `strength_and_time_in_platform_units` | 振幅向下取整 |
| `strength_and_time_in_platform_units` | 毫秒不封顶 |
| `strength_and_time_in_platform_units` | 单马达取小的 |
| `triggers_dpad_and_sticks` | 浏览器摇杆 y 不翻 |
| `only_standard_pads_are_read` | 浏览器不看布局 |
| `triggers_dpad_and_sticks` | 浏览器十字键上下反了 |
| `standard_buttons_become_the_same_reading_as_on_mac` | 浏览器 Menu / Options 反了 |
| `triggers_dpad_and_sticks` | 浏览器扳机当成开关 |
| `rumble_in_browser_units` | 浏览器强弱马达反了 |
| `each_device_has_its_own_state_from_buttons_and_axes` | Android 按设备不带按钮 |
| `landing_and_getting_hurt_rumble_the_first_players_pad` | 落地不震 |
| `landing_and_getting_hurt_rumble_the_first_players_pad` | 受伤不震 |
| `landing_and_getting_hurt_rumble_the_first_players_pad` | 站着也算落地 |
| `the_second_pad_moves_the_second_player` | 第二个角色读第一个玩家的动作 |
| `the_second_player_also_has_keys_and_comes_back_after_a_fall` | 第二个角色掉下去不回来 |
| `a_stick_pushed_a_little_walks_slowly` | 不设默认绑定 |
| `landing_and_getting_hurt_rumble_the_first_players_pad` | 跳不问动作的刚按下 |
| `a_stick_pushed_a_little_walks_slowly` | 移动不乘轴的值 |
| `input.test.tsx › 键盘的键查所有玩家，手柄的键只查同一个玩家，同一个动作里的不算` | 手柄的键查所有玩家 |
| `input.test.tsx › 键盘的键查所有玩家，手柄的键只查同一个玩家，同一个动作里的不算` | 同一个动作里的也算冲突 |
| `input.test.tsx › 换一个键得到新的表，原来的不动` | 末尾之后不加键 |
| `input.test.tsx › 换一个键得到新的表，原来的不动` | null 不去掉键 |
| `input.test.tsx › Escape 取消` | Escape 也拿去绑 |
| `input.test.tsx › 按任意键来绑：下一个按下的键换上去` | 自动重复也算 |
| `input.test.tsx › 按任意键来绑：下一个按下的键换上去` | 没在绑键也吞按键 |
| `input.test.tsx › 冲突时先问：确认就对调，取消就不变` | 确认时不对调 |
| `input.test.tsx › 没冲突直接换，有冲突不换并给出冲突的位置` | 有冲突也换 |
| `input.test.tsx › 存的表按默认的对一遍` | 存档里的键不查类型 |
| `input.test.tsx › 结构不对、版本不对的整份不要` | 存档不看版本 |
| `input.test.tsx › 存的表按默认的对一遍` | 存档的模拟量也用 |
| `input.test.tsx › 读完之前改的以改的为准` | 读档盖掉刚改的 |
| `input.test.tsx › 改了就存，下次读回来` | 改了不存 |
| `input.test.tsx › 恢复默认` | 恢复默认不结束绑键 |
| `input.test.tsx › 冲突时先问：确认就对调，取消就不变` | 等回答时不算在绑键 |
| `input.test.tsx › 手柄接入、拔出、换玩家` | 同一个手柄报两遍留两份 |
| `input.test.tsx › 动作的状态来自 action 事件` | 动作状态不分玩家 |
| `input.test.tsx › 改了就存，下次读回来` | 读完不置 ready |
| `controls.test.tsx › 面板的提示：平时、等按键、有冲突` | 面板提示不看冲突 |
| `controls.test.tsx › 面板的提示：平时、等按键、有冲突` | 冲突不标是谁的 |
| `controls.test.tsx › 第二个角色出现在起点右边，金币和终点不认它` | 第二个角色也吃金币 |
| `controls.test.tsx › 键名显示得短一点` | 方向键不换成箭头 |
| `controls.test.tsx › 改键面板：点按钮打开，列出跳跃的键，点一个等按键` | 关面板不取消绑键 |
| `controls.test.tsx › 改过的键存在设备上：重开后面板里是改过的，恢复默认后存的也是默认的` | 面板不读存的绑定 |
| `input.test.tsx › 存的表按默认的对一遍` | 存档里种类变了也用 |

## 还没手测

这一项没有起模拟器，也没有开窗口跑，下面这些只有编译检查和纯函数的单元测试：

- 真手柄：macOS、iOS、Android、浏览器上按设备的状态、接入和拔出、两个手柄分给两个玩家。
- 震动：四个平台都没有在真手柄上震过。`CHHapticEngine` 的建法只在 Mac 上测了「建得出事件和模式」，没有连着手柄放过。
- Android 的 `vibratePad` 只编过，没跑过；`InputDevice` 的振动器要不要 `VIBRATE` 权限没验证（没权限时接住异常、记一条日志）。
- 浏览器的手柄读数（标准布局）只测了换算。
- 平台跳跃的改键面板、第二个角色没有在窗口里看过，只有 `nsh test` 和 Rust 的单元测试。
