# 联网进阶：权威服务器、预测和回滚、延迟补偿、鉴权

> 前置：联网（[networking.md](networking.md)）、输入映射（[input-actions.md](input-actions.md)）、2D 玩法补齐（[gameplay2d.md](gameplay2d.md)）。

## 目标

现在的联网是「各自广播自己、远端插值显示」：谁都能说自己在哪，排行榜谁都能交、名字不归谁。这一项补上联网游戏常用的几件：

- 权威服务器：服务端跑一份模拟，客户端只发输入，状态以服务端为准。
- 客户端预测和回滚：自己的角色不等服务端，按自己的输入先走；服务端的结果回来以后对账，不一样就回到那个 tick 重放，显示上平滑修正。
- 延迟补偿：服务端判「拍人」时回到拍的人当时看到的那个时刻。
- 鉴权：连接带 token，服务端用 HMAC 校验，过期、伪造、重放都拒；排行榜的成绩按账号归属，一个名字归一个账号。

**验收标准**：
- 服务端：`nge-relay` 里加权威房间，按 30 Hz 推进，收按 tick 编号的输入，给每个人发增量快照（只发和他确认过的那个 tick 比变了的字段）。客户端在权威房间里只发输入。
- 确定性：固定步长、输入按 tick 排序；同样的输入序列得到逐位一样的状态（测试比的是 `to_bits`）。为什么不用 rapier 写清楚。
- 预测和回滚的核心（输入缓冲、状态历史的环形缓冲、对账、平滑修正、对时）是纯逻辑，脱离渲染测；用模拟网络（延迟、抖动乱序、丢包）验证预测收敛到服务端的状态。
- 鉴权：token 签发和校验（过期、伪造、重放都拒）；密钥从环境变量读，代码里没有；本机开发的免鉴权模式要显式打开（`--dev`）。排行榜成绩按鉴权后的身份归属。
- TSX：场景的 `net` 能选模式、带 token、指定预测的精灵；`net` 事件带统计（延迟、回滚次数、预测误差）；`netScore` 交成绩。
- 演示：平台跳跃按 M 进权威联机竞技场，HUD 显示延迟、回滚次数、预测误差、被拍次数。

## 做法

### 结构

```
nge-net
  auth.rs          token 签发、校验，握手的证明（HMAC-SHA256）
  arena/mod.rs     竞技场的确定性模拟：角色的跑、跳、落到平台上，拍人的判定
  arena/delta.rs   输入和快照的线上格式，增量编解码，坏数据校验
  arena/room.rs    服务端的一个权威房间：输入队列、按 tick 推进、历史、延迟补偿、按人做增量
  arena/client.rs  客户端：解快照、预测、对账、对时
  rollback.rs      通用的预测器（Model trait）和对时（Pacer），和竞技场无关
  simnet.rs        测试用的模拟网络
  authority.rs     ECS：Predicted 组件、每帧走 tick 发输入、写位置、发统计（feature ecs）
nge-relay
  hub.rs           Hello 鉴权、权威房间的进出和 step、走连接交成绩
  server.rs        30 Hz 调 Hub::step；鉴权时 HTTP 不收分
  bin/nge-relay    --dev 或 NGE_AUTH_SECRET 二选一
  bin/nge-token    签 token
```

权威房间的逻辑放在 `nge-net`（纯逻辑，客户端测试里能直接拿来当服务端），`nge-relay` 只负责把它接到连接上、按时间调 `step`。`nge-net` 的纯逻辑部分没有平台相关的依赖，客户端照样能编到 wasm；服务端（`nge-relay`，tokio 的多线程运行时）不编 wasm。

### 协议

协议版本升到 2（`Hello` 多了字段，旧客户端连上来回 `Version`）。在原来的消息后面加：

- 客户端 → 服务端：`Hello { version, room, name, mode, auth }`（`mode` 是 `Relay` / `Authority`，`auth` 是 `{ token, proof }`）、`Input(InputMsg)`、`Score { board, score }`。
- 服务端 → 客户端：`Challenge { nonce, required }`（连上就发）、`World(WorldSnapshot)`、`Ranked { board, rank }`；错误码加 `Unauthorized`、`WrongMode`、`NameTaken`。
- 一个房间的模式由第一个进来的人定；模式不同的人进不来（`WrongMode`），在中继房间里发输入回 `WrongMode`。

### 确定性

- 竞技场的模拟（`arena::step`）只看一个角色自己和它的输入，按固定步长 1/30 秒走一步；角色之间的影响（拍人）只在服务端算，被拍的次数不参与客户端对账。
- 每个 tick 用的输入按 tick 编号放进队列（`BTreeMap`），乱序到的照样排好；服务端算到某个 tick 时取那个 tick 的输入，没到的沿用上一个。
- 只用 IEEE 754 的加减乘除、比较、`min` / `max` / `clamp`：这些在各平台上结果逐位一样。不用三角函数、`sqrt`（只在显示用的误差里用）、`mul_add`；Rust 不会自己把乘加合成 FMA。拍人的距离用平方比较。
- 为什么不用 rapier：rapier 默认的求解结果跨平台不保证一样（SIMD 路径、并行求解、编译器对浮点的取舍都可能让最后几位不同），它的 `enhanced-determinism` 要求关掉 SIMD 和并行、所有平台同一个编译设置，代价大，而且平台跳跃主场景里还有关节、动态刚体、移动平台这些都得一起确定。预测和回滚要求客户端重放出和服务端逐位一样的结果，差一位就会一直回滚；所以竞技场用一份手写的、只用基本运算的运动学模拟。主场景的「中继」联机照旧。
- 测试：`arena::the_same_inputs_give_the_same_state_bit_for_bit`（600 步伪随机输入跑两遍逐位比）、`room::the_same_inputs_give_the_same_world`（三个人 300 个 tick）。

### 权威房间（`ArenaRoom`）

- `join` 在出生点放角色（按 id 错开），`leave` 去掉，最多 16 人（还受中继的房间人数上限管）。
- `input(id, msg)`：`msg` 是从 tick `first` 起的连续输入（最多 32 个，客户端每次重发还没确认的最新 16 个，丢一条靠下一条补）。已经算过的 tick、超前 60 个 tick（2 秒）以上的丢掉；每人最多排 64 个；同一个 tick 先到的为准。`ack` 是客户端解出来的最新快照的 tick，当下一个快照的基准；`view` 是客户端正显示的远端 tick。
- `step()`：tick 加一，每人取输入走一步，处理拍人，记进历史（最近 32 个 tick），给每个人发快照：基准是他确认过、还在历史里的那个 tick，否则发全量。快照里的 `ack` 是服务端用到的他最新的真输入的 tick。
- 服务端每 1/30 秒调一次（tokio 的 `interval`，落后了连着补，保持和墙上时间对齐）。

### 增量快照（`arena/delta.rs`）

- 角色的状态：`x, y, vx, vy, ground, cool`（拍人冷却）、`tagged`（被拍次数）。
- `diff(base, now)`：基准里有、没变的角色整个不发；变了的只带变了的字段（`Field::X(f32)` 这样一个字段一条，浮点按位比）；基准里有、现在没有的列进 `removed`；新出现的带全 7 个字段。
- `apply(base, …)`：套在基准上；新角色字段不全（`MissingBase`）、同一个角色出现两次（`Duplicate`）都是错，整个快照丢掉。
- 都站着不动时一个快照不到 12 字节，8 个人的全量 160 多字节。
- 校验（解码时就做，坏的不往外交）：坐标不是有限数或超过 ±1e6、速度超过 ±1e4、角色或删除超过 64 个、一个角色没有字段 / 字段超过 7 个 / 字段重复、基准不比自己旧。输入：条数 1…32、只认 4 个按键位、`first + 条数` 不溢出。帧长先按上限挡掉。

### 预测和回滚（`rollback.rs`、`arena/client.rs`）

- `Predictor<M: Model>`：`predict(input)` 走一个 tick，把 `(tick, 输入, 走完的状态)` 记进环形缓冲（72 个 tick）。
- `reconcile(at, auth)`：比已确认的旧或重复的不管；丢掉更旧的历史；那个 tick 的预测和权威的一样（`Model::same`）就确认；不一样就回到权威状态，把之后还没确认的输入按顺序重放、改写历史；手里没有那个 tick（服务端跑到前面了、历史被挤掉了）就直接用权威状态从头来。
- 平滑修正：回滚前后显示位置的差记成偏移，显示时加上，之后按时间常数 0.1 秒指数衰减到 0；差超过 200 直接跳过去。显示位置在上一个和这一个 tick 之间按时间插值。
- 统计：回滚次数、重放的 tick 数、最近一次和最大的预测误差、从头来的次数。
- 对时（`Pacer`）：客户端要超前服务端「一个往返 + 2 个 tick」（输入过去要半个往返，快照回来本身旧了半个往返）。超前不够快 10%，超前多 2 个以上慢 10%；差 3 个以上一下子补上（刚连上、从头来之后），多 15 个以上停着等。一帧最多补 8 个 tick。往返时间用心跳量（`NetClient::rtt`，`Ping` 到 `Pong`），还没量到按 100 ms。
- `ArenaClient`：留最近 64 个解出来的世界当基准；比已有的旧的快照不要；解出世界后对账自己的角色，别人的角色交给原来的插值缓冲（时间戳是 `tick × 33.3 ms`，key 是 `hero`）。每走一个 tick 发一条 `InputMsg`（`view` = 最新 tick − 3，和插值的 100 ms 延迟对上）。

### 延迟补偿

- 拍人（`TAG` 键）：服务端处理某人在 tick N 的拍人时，拿他 `view` 那个 tick 的历史里别人的位置，和他自己在 N−1 的位置比，中心距离在 48 以内的最近一个（一样近的取 id 小的）被拍一次。
- 回溯最多 9 个 tick（300 ms），再旧的按上限截，免得一个延迟很大（或者谎报 `view`）的人拍到很久以前的位置。`view` 比现在新的按现在算。
- 测试：`room::tags_are_judged_where_the_attacker_saw_the_target`：按现在的位置够不着、回溯到看到的时候拍中、看到的太旧按上限截以后够不着。

### 鉴权（`auth.rs`）

- 凭据：`v1.<声明>.<签名>.<会话密钥>`，都是 base64url。声明是 postcard 编的 `{ sub, name, exp }`（账号、名字、过期的 Unix 秒）；签名 = HMAC-SHA256(密钥, "nge-token-v1" ‖ 声明)；会话密钥 = HMAC-SHA256(密钥, "nge-key-v1" ‖ 声明)。线上的 token 只有前三段，会话密钥不出客户端。
- 握手：服务端每条连接先发 `Challenge { nonce }`（16 字节；鉴权时 = HMAC(密钥, 启动时刻 ‖ 连接序号) 的前 16 字节，同一次启动里不重复、重启后也不一样）。带凭据的客户端等到它，回 `Hello { …, auth: { token, proof } }`，`proof` = HMAC(会话密钥, "nge-hello-v1" ‖ nonce ‖ 房间名)。不带凭据的客户端照旧连上就发 `Hello`。
- 校验顺序：长度、格式（三段、版本）→ 签名（常数时间比较）→ 解声明、查账号名字合规 → 没过期、有效期不超过 30 天 → 证明。任何一步不过都回 `Unauthorized` 并断开。
- 为什么能防重放：抓到一次握手（token + 证明）拿到别的连接上发，随机数不一样，证明对不上；换房间也对不上；只抓到 token 没有会话密钥，算不出证明。token 过期后整个作废。
- 房间里的名字以 token 里的为准（`Hello` 里写的不算）；客户端收到 `Unauthorized` 后停止重连，换了目标（新 token）才再连。
- 密钥：中继从环境变量 `NGE_AUTH_SECRET` 读（至少 16 字节），`Config::auth` 也可以直接给；代码和仓库里没有任何密钥，测试用测试自己写的密钥。`Secret`、`Credential` 的 `Debug` 不打印内容。
- 免鉴权：`nge-relay --dev` 才不鉴权；既没有 `--dev` 也没有密钥时不起，两个都有也不起。库里的 `Config::default()` 是不鉴权的（测试和嵌进别的程序用），只有命令行要求显式选。
- 签发：没有账号系统、没有登录界面，只给一个工具和一个函数：`nge-token --sub 账号 --name 名字 [--ttl 秒]`（默认 1 小时，最多 30 天）和 `Secret::issue`。谁该拿哪个账号由调用的人（自己的登录服务、运维）决定。

### 排行榜的归属

- 成绩可以走联机连接交：`Score { board, score }`，服务端记在这条连接的身份名下（鉴权时是 token 里的账号和名字，开发模式是 `Hello` 里的名字），回 `Ranked { board, rank }`。
- 一个名字第一次被哪个账号交过就归它（最多记 1 万个名字，内存里，重启清空）；别的账号、不带账号的再用这个名字交，回 `NameTaken`。
- 配了密钥时 HTTP 的 `POST /scores` 回 403（HTTP 请求没有鉴权，不知道是谁），`GET` 照常。
- 平台跳跃在线时用 `netScore` 交、收到 `ranked` 再拿榜首；没在线时照旧走 HTTP。

### 加密

不在中继里做 TLS：把中继放在 TLS 反向代理后面，客户端连 `wss://`。原生客户端本来就支持 `wss://`（rustls + webpki 根证书），浏览器也是。比如 Caddy：

```
game.example.com {
  reverse_proxy 127.0.0.1:9100
}
```

Caddy 自动申请证书、转发 WebSocket 升级；中继只听 127.0.0.1。场景里写 `net.url = "wss://game.example.com"`，排行榜的 HTTP 地址跟着变成 `https://`（`relayHttp`）。不走 TLS 时 token 和证明是明文，但会话密钥不上线，抓包拿到的东西不能用来连。

### ECS 和场景

- 场景的 `net` 多了 `mode`（`"relay"` / `"authority"`，不认得的按 `relay` 并记一行警告）、`token`（不写时原生读环境变量 `NGE_TOKEN`）、`predict`（自己的角色的精灵 id，带上 `Predicted` 组件）。
- 资源 `Arena`：在权威房间里在线时有一个 `ArenaClient`，断线、换房间时清掉。
- 系统：收到 `World` 时解快照、对账、把别人的角色喂给插值缓冲；`PostUpdate` 的 `NetSet::Apply` 里按真实时间（`Time::real_delta`，场景暂停不停网络）走 tick、发输入、写 `Predicted` 的位置；每 0.5 秒发一次统计。权威房间里不广播 `NetSync`。
- 输入从动作来：`move` 推过 0.3 算左右、`jump`、`tag`。
- TSX 的事件：`status` 在权威房间里带 `arena`（宽度、平台、角色大小，TSX 照着画，和服务端是同一份常量）；`stats`（`rtt`、`rollbacks`、`resets`、`error`、`max_error`、`lead`、`tick`、`tagged`、`dropped`）；`ranked`。command：`netScore { board, score }`。`netRoom()` 多了 `arena()`、`stats()`。

### 平台跳跃

- 按 M 进出权威联机竞技场（桌面上，试玩时不行）：960 宽的地面、三块平台，自己的角色是橙色，别人的是半透明蓝色、头顶名字。← → 跑、空格跳、J 拍人（`tag` 动作，手柄 X，可以改键）。
- HUD：「权威联机竞技场 … · 延迟 40 ms · 回滚 2 次 · 预测误差 1.5 · 被拍 1 次 · 在线 2 人」。
- 代码：`examples/platformer/arena.ts`（纯函数，`arena.test.tsx` 测）和 `game.tsx`。

## 不做

- 用 rapier 的权威模拟、主场景（关节、动态刚体、移动平台、敌人）的权威联机：原因见「确定性」。主场景照旧是中继模式。
- 远端角色的预测（外推）：远端照旧按快照插值，晚 100 ms。
- 服务端对输入的合理性检查以外的防作弊：输入只有 4 个按键位，速度、位置都是服务端算的，客户端改不了；但自动按键（脚本）防不了。
- 账号系统、登录界面、token 吊销：只有签发工具和函数；要提前作废只能换密钥（所有 token 一起作废）。
- 中继自己终止 TLS：交给反向代理。
- 回滚时重放别人的输入（全量回滚）：别人的角色由服务端算，客户端只回滚自己的。
- 多进程、多机的权威房间，房间状态持久化。

## 验收

- `cargo test --workspace -j 4`、`cargo test -p nge-scene --no-default-features -j 4`、platformer / sanguo / editor 逐个 `nsh test` 和 `nsh check`、iOS 的 `cargo check`、`tools/check-wasm.sh`、`tools/check-android.sh` 通过。
- 新加的测试：
  - 鉴权（`auth`）：签了能过、密钥太短不收、过期、伪造（别的密钥、改声明、格式不对）、重放（换随机数、换房间、瞎猜的证明）、每条连接随机数不一样、不合规的声明不签。
  - 模拟（`arena`）：站稳、跑跳落回、从下面穿过平台落到上面、墙、逐位确定、拍人的冷却和选人。
  - 增量（`arena::delta`）：只带变了的、静止时很小、要有基准、坏快照（NaN、无穷、越界、字段重复、空、太长、条目太多、基准不旧）、坏输入。
  - 房间（`arena::room`）：输入按 tick 用、沿用上一个、乱序排好、旧的和太超前的丢掉；按确认的基准做增量、基准掉出历史发全量；延迟补偿三种情况；确定性；人数上限。
  - 预测（`rollback`）：输入缓冲、确认、回滚重放、平滑修正和跳过、从头来、显示插值、对时。
  - 收敛（`arena::client`，模拟网络）：干净的线路 0 次回滚、超前在目标附近；延迟 60 ms + 抖动 120 ms（乱序）+ 丢包 15% 下有回滚，停手后预测的和显示的都和服务端逐位一致；断线 1 秒后恢复；没基准的快照丢掉。
  - 客户端（`client`）：带凭据的等随机数再发证明、换连接证明跟着变、凭据坏了不连、`Unauthorized` / `WrongMode` 不再重连、心跳量往返、快照只在权威房间收、交成绩。
  - ECS（`authority`）：自己的角色按预测跑在服务端前面、别人的按快照插值、统计每秒两次、权威房间里不广播位置、动作变成按键位。
  - 中继（`hub`）：每条连接有随机数、要鉴权时只有签了且证明对的进得来（不带、重放、过期、伪造都拒，名字用 token 里的）、成绩归账号、权威房间推进和发快照、模式不对报错、走光了房间去掉；HTTP 鉴权时不收分。
  - 排行榜：名字归第一个用它的账号。
  - 集成（`nge-relay/tests/relay.rs`）：真的中继（要鉴权）、真的客户端：签了的进权威房间收到全量快照，没 token 的和别的密钥签的被拒且停下。
  - 场景：`mode` / `token` / `predict` 接上，不认得的模式按 `relay`。
  - TSX：`netRoom` 的 `arena()` / `stats()`、`netScore`；`arena.ts` 的场景、`net`、HUD 统计。
- 变异验证：见下面的表。
  - 对账（`arena::client`）：任何一个字段不同都算预测错了。

### 本机试

```sh
# 不鉴权（本机开发）
cargo run -p nge-relay -- --dev
cd examples/platformer && nsh run        # 开两个，都按 M 进竞技场

# 要鉴权
export NGE_AUTH_SECRET="$(openssl rand -hex 32)"
cargo run -p nge-relay                                                     # 起中继
cargo run -p nge-relay --bin nge-token -- --sub alice --name 小红           # 打印一张凭据
NGE_TOKEN="<上面打印的>" nsh run                                           # 在 examples/platformer 里
```

要鉴权时没带 token 的客户端连不上（连不上的原因在日志里），通关成绩走联机连接交，HTTP 的 `POST /scores` 回 403。

## 变异验证

脚本逐条临时改坏，跑相应 crate 的测试，确认有测试报错，再改回原样并 touch。28 条里 27 条第一次就抓到；第 23 条（对账时不比 x）没抓到：模拟网络下 x 错了 vx 一般也错，靠 vx 也会回滚，单比 x 的漏洞测不出来，补了逐个字段改的测试以后抓到。

| # | 文件 | 改成 | 结果 | 报错的测试 |
|---|---|---|---|---|
| 1 | `nge-net/src/auth.rs` | 过期不拒 | 抓到 | `expired_tokens_are_refused` |
| 2 | `nge-net/src/auth.rs` | 不验签名 | 抓到 | `forged_and_tampered_tokens_are_refused` |
| 3 | `nge-net/src/auth.rs` | 有效期不设上限 | 抓到 | `expired_tokens_are_refused` |
| 4 | `nge-relay/src/hub.rs` | 每条连接同一个随机数（可重放） | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in` |
| 5 | `nge-relay/src/hub.rs` | 要鉴权时不带 token 也放进来 | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in` |
| 6 | `nge-relay/src/hub.rs` | 名字用 Hello 里的、不用 token 里的 | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in` |
| 7 | `nge-relay/src/hub.rs` | 模式不同也能进 | 抓到 | `authority_rooms_step_and_send_snapshots` |
| 8 | `nge-relay/src/scores.rs` | 名字不归账号 | 抓到 | `scores_sent_on_the_connection_belong_to_the_account`、`a_name_belongs_to_the_first_account_that_uses_it` |
| 9 | `nge-relay/src/http.rs` | 鉴权时 HTTP 照样收分 | 抓到 | `scores_can_be_submitted_and_read` |
| 10 | `nge-net/src/arena/room.rs` | 太超前的输入不丢 | 抓到 | `inputs_are_used_by_tick_and_the_last_one_repeats` |
| 11 | `nge-net/src/arena/room.rs` | 没到的输入不沿用上一个 | 抓到 | `inputs_are_used_by_tick_and_the_last_one_repeats`、`inputs_arriving_out_of_order_are_sorted_by_tick`、`snapshots_are_deltas_against_what_you_acknowledged` |
| 12 | `nge-net/src/arena/room.rs` | 不做延迟补偿 | 抓到 | `tags_are_judged_where_the_attacker_saw_the_target` |
| 13 | `nge-net/src/arena/room.rs` | 回溯不设上限 | 抓到 | `tags_are_judged_where_the_attacker_saw_the_target` |
| 14 | `nge-net/src/arena/room.rs` | 不按确认的基准做增量（总发全量） | 抓到 | `snapshots_are_deltas_against_what_you_acknowledged` |
| 15 | `nge-net/src/arena/delta.rs` | 没变的字段也发 | 抓到 | `a_still_world_is_a_tiny_snapshot`、`unchanged_heroes_are_not_sent_and_changed_ones_carry_only_what_changed`、`snapshots_are_deltas_against_what_you_acknowledged` |
| 16 | `nge-net/src/arena/delta.rs` | 坐标不查 NaN、越界 | 抓到 | `bad_snapshots_are_rejected` |
| 17 | `nge-net/src/arena/delta.rs` | 字段重复不查 | 抓到 | `bad_snapshots_are_rejected` |
| 18 | `nge-net/src/arena/delta.rs` | 没基准的增量当新角色 | 抓到 | `deltas_need_their_base` |
| 19 | `nge-net/src/rollback.rs` | 回滚时不重放输入 | 抓到 | `a_mismatch_rolls_back_and_replays_the_unconfirmed_inputs`、`corrections_are_smoothed_not_teleported` |
| 20 | `nge-net/src/rollback.rs` | 回滚不平滑（瞬移） | 抓到 | `corrections_are_smoothed_not_teleported` |
| 21 | `nge-net/src/rollback.rs` | 对账不比状态（从不回滚） | 抓到 | `a_long_stall_recovers`、`prediction_converges_under_delay_loss_and_reordering`、`a_mismatch_rolls_back_and_replays_the_unconfirmed_inputs` |
| 22 | `nge-net/src/arena/client.rs` | 超前量不看往返时间 | 抓到 | `a_clean_steady_link_predicts_without_rollbacks` |
| 23 | `nge-net/src/arena/client.rs` | 对账不比 x | 抓到（补了测试） | `any_differing_field_counts_as_a_misprediction` |
| 24 | `nge-net/src/arena/mod.rs` | 从下面也能落到平台上 | 抓到 | `standing_on_the_floor_is_stable`、`runs_jumps_and_lands_back`、`the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |
| 25 | `nge-net/src/client.rs` | 带凭据的不等随机数就发 Hello | 抓到 | `credentials_wait_for_the_challenge_and_prove_this_connection` |
| 26 | `nge-net/src/client.rs` | 鉴权不过照样重连 | 抓到 | `a_version_mismatch_or_bad_hello_stops_retrying` |
| 27 | `nge-net/src/sync.rs` | 权威房间里照样广播位置 | 抓到 | `the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |
| 28 | `nge-net/src/authority.rs` | 预测不读动作 | 抓到 | `the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |

## 修过的问题

- 往返时间一开始用「输入发出到快照确认它」量：超前得越多，确认回来得越晚，量出来的往返越大，超前量跟着越涨。改成心跳（`Ping` / `Pong`）量，和超前量无关。
- 延迟补偿的边界：客户端报的 `view` 可能比服务端当前还新，也可能早过回溯上限，开头只防了一头。改成把 `view` 夹在 `[tick - 1 - MAX_REWIND, tick - 1]` 里（早期 tick 不够减时用饱和减）；测试改成「追人」（`chase`），分别看没超上限、刚好上限、超上限三种。
- 两个人离拍的人一样近时选谁要固定，测试原来预期错了（按编号小的选）。
- 对时（`Pacer`）的测试原来比确切的步数，浮点累加差一点就多走或少走一步：改成比范围。
- 客户端测试里签 token 用的「现在」离过期太远，超过了有效期上限被当成不合规的声明：换一个近一点的时间。
- 动作只有在绑定表里声明过才有状态，测试里直接发 `action` 读不到：测试先装上声明了 `move` / `jump` / `tag` 的绑定表；平台跳跃的 `controls.json` 加了 `tag`（J 键、手柄 X）。
- 测试里的远端实体在连上之前就因为「不在房间里」被清掉：加 `KeepOnLeave`。
- `controls.json` 加了 `tag` 以后，改键面板的夹具（`remap-fixture.json`，TSX 和 Rust 两边共用）还是旧的默认绑定，`remap_fixture_drives_the_engine` 报错：夹具跟着加上。
- 第 23 条变异没抓到（见上表）：补了测试。

## 还没手测

下面这些只有自动测试，没有真的开窗口、真机或真的网络试过：

- 两个窗口同时 `nsh run` 进竞技场，看预测、插值、拍人、HUD 的数字（延迟、回滚、误差）在真实帧率下是否顺。
- 真实的公网延迟和丢包（测试里是模拟网络）；手机网络下的抖动。
- 要鉴权的中继配 token 在真窗口里连（集成测试里是真的中继和真的客户端，但没有渲染）；token 过期后重连的提示。
- wss：反向代理（Caddy / nginx）终止 TLS 再转给中继，没有实际配过证书试。
- iOS、Web、Android 上的竞技场：只过了编译检查，没在模拟器、浏览器、真机上跑。
- 手柄按 X 拍人、改键界面里改「拍」。
