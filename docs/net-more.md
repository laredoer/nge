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
  startup.rs       启动时的选择（纯函数）：鉴权开不开、nge-token 的参数、时钟和随机数种子
  bin/nge-relay    --dev 或 NGE_AUTH_SECRET 二选一
  bin/nge-token    签 token
```

权威房间的逻辑放在 `nge-net`（纯逻辑，客户端测试里能直接拿来当服务端），`nge-relay` 只负责把它接到连接上、按时间调 `step`。`nge-net` 的纯逻辑部分没有平台相关的依赖，客户端照样能编到 wasm；服务端（`nge-relay`，tokio 的多线程运行时）不编 wasm。

### 协议

协议版本升到 2（`Hello` 多了字段，旧客户端连上来回 `Version`）。在原来的消息后面加：

- 客户端 → 服务端：`Hello { version, room, name, mode, auth }`（`mode` 是 `Relay` / `Authority`，`auth` 是 `{ token, proof }`）、`Input(InputMsg)`、`Score { board, score }`。
- 服务端 → 客户端：`Challenge { nonce, required }`（连上就发）、`World(WorldSnapshot)`、`Ranked { board, rank }`；错误码加 `Unauthorized`、`WrongMode`、`NameTaken`、`ScoresFull`（名字归属表满了）、`TooManyBoards`（这个账号建的新榜够多了）。
- 一个房间的模式由第一个进来的人定；模式不同的人进不来（`WrongMode`），在中继房间里发输入回 `WrongMode`。

### 确定性

- 竞技场的模拟（`arena::step`）只看一个角色自己和它的输入，按固定步长 1/30 秒走一步；角色之间的影响（拍人）只在服务端算，被拍的次数不参与客户端对账。
- 每个 tick 用的输入按 tick 编号放进队列（`BTreeMap`），乱序到的照样排好；服务端算到某个 tick 时取那个 tick 的输入，没到的沿用上一个。
- 只用 IEEE 754 的加减乘除、比较、`min` / `max` / `clamp`：这些在各平台上结果逐位一样。不用三角函数、`sqrt`（只在显示用的误差里用）、`mul_add`；Rust 不会自己把乘加合成 FMA。拍人的距离用平方比较。
- 为什么不用 rapier：rapier 默认的求解结果跨平台不保证一样（SIMD 路径、并行求解、编译器对浮点的取舍都可能让最后几位不同），它的 `enhanced-determinism` 要求关掉 SIMD 和并行、所有平台同一个编译设置，代价大，而且平台跳跃主场景里还有关节、动态刚体、移动平台这些都得一起确定。预测和回滚要求客户端重放出和服务端逐位一样的结果，差一位就会一直回滚；所以竞技场用一份手写的、只用基本运算的运动学模拟。主场景的「中继」联机照旧。
- 测试：`arena::the_same_inputs_give_the_same_state_bit_for_bit`（600 步伪随机输入跑两遍逐位比）、`room::the_same_inputs_give_the_same_world`（三个人 300 个 tick）。这两个测试都是同一台机器上跑两遍比，只说明没有依赖跑的顺序、没有未初始化的东西；**没做跨平台对照**（比如 x86_64 和 arm64、wasm 各跑一遍比结果），「各平台逐位一样」目前是按 IEEE 754 推出来的，没有实测。

### 权威房间（`ArenaRoom`）

- `join` 在出生点放角色（按 id 错开），`leave` 去掉，最多 16 人（还受中继的房间人数上限管）。
- `input(id, msg)`：`msg` 是从 tick `first` 起的连续输入（最多 32 个，客户端每次重发还没确认的最新 16 个，丢一条靠下一条补）。已经算过的 tick、超前 60 个 tick（2 秒）以上的丢掉，所以每人最多排 60 个；同一个 tick 先到的为准。`ack` 是客户端解出来的最新快照的 tick，当下一个快照的基准；`view` 是按下最后一个输入时客户端看到的远端 tick，队列里每个输入都记着自己的 `view`（第一次收到时一般就是最后一个；重发补上的按每个 tick 早一个推算），拍人按那个输入自己的 `view` 回溯，不按后来的消息。
- `step()`：tick 加一，每人取输入走一步，处理拍人，记进历史（最近 32 个 tick），给每个人发快照：基准是他确认过、还在历史里的那个 tick，否则发全量。
- tick 是 `u32`，30 Hz 下 4.5 年才到头；到头了 `step` 不再走（不回绕、不 panic），客户端的 `Predictor` 一样停住。房间在人走光时去掉，下次有人进来从 0 开始。
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
- 对时（`Pacer`）：客户端要超前服务端「一个往返 + 2 个 tick」（输入过去要半个往返，快照回来本身旧了半个往返）。超前不够快 10%，超前多 2 个以上（`lead > target + 2`）慢 10%；比想要的少 3 个以上（`lead + 3 < target`，也就是少 4 个起）一下子补上（刚连上、从头来之后），多 15 个以上（`lead > target + 15`，多 16 个起）停着等。`Pacer` 一帧最多走 8 个 tick；一下子补的时候一帧最多 16 个。
- 想要的超前有上限 `MAX_TARGET_LEAD` = min(历史 72 − 15 − 余量 5, 服务端收输入的超前上限 60) = 52 个 tick，对应往返大约 1.6 秒。往返再大时超前量停在 52：输入晚到、服务端沿用旧输入，客户端靠回滚对上，不会每个快照都找不到当时的预测而从头来（不夹的话往返 2.4 秒以上每个快照都从头来）。
- 往返时间用心跳量（`NetClient::rtt`，`Ping` 到 `Pong`，每 2 秒一次），取最近 5 次的中位数：偶尔一次卡顿不把超前量带跑，一直变慢 3 次以上才跟上。还没量到按 100 ms。
- `ArenaClient`：留最近 64 个解出来的世界当基准；比已有的旧的快照不要；解出世界后对账自己的角色，别人的角色交给原来的插值缓冲（时间戳是 `tick × 33.3 ms`，用 `f64` 毫秒，不经过中继模式快照里的 `u32`，开 50 天以上也不会卡住；key 是 `hero`）。每走一个 tick 发一条 `InputMsg`（`view` = 最新 tick − 3，和插值的 100 ms 延迟对上）。快照里不带「服务端用到了你哪个输入」（客户端用不上，省带宽）。

### 延迟补偿

- 拍人（`TAG` 键）：服务端处理某人在 tick N 的拍人时，拿他 `view` 那个 tick 的历史里别人的位置，和他自己在 N−1 的位置比，中心距离在 48 以内的最近一个（一样近的取 id 小的）被拍一次。
- 要回溯多少：按下拍的那个 tick 比客户端看到的最新快照超前 `ceil(往返 / DT) + 2` 个，看到的远端又比最新快照落后 3 个，所以是 `ceil(往返 / DT) + 5`。上限 `MAX_REWIND` = 300 ms 的 9 个 tick + 取整 1 + 2 + 3 = **15 个 tick（500 ms）**，**覆盖往返 300 ms 以内**；往返更大的人按上限截（拍到的是比他看到的晚一点的位置），免得一个延迟很大（或者谎报 `view`）的人拍到很久以前的位置。`view` 比现在新的按现在算。
- 测试：`room::tags_are_judged_where_the_attacker_saw_the_target`：按现在的位置够不着、回溯到看到的时候拍中、刚好回溯 15 个拍中、要回溯 16 个按上限截以后够不着；`arena::client::lag_compensation_covers_a_200ms_round_trip`：真的 `ArenaClient` + 模拟网络，往返 200 ms 和 300 ms，在自己看到对方刚要跑出范围时按拍，拍中（服务端算到那个 tick 时对方早跑远了）。

### 鉴权（`auth.rs`）

- 凭据：`v1.<声明>.<签名>.<会话密钥>`，都是 base64url。声明是 postcard 编的 `{ sub, name, iat, exp }`（账号、名字、签发和过期的 Unix 秒）；签名 = HMAC-SHA256(密钥, "nge-token-v1" ‖ 声明)；会话密钥 = HMAC-SHA256(密钥, "nge-key-v1" ‖ 声明)。线上的 token 只有前三段，会话密钥不出客户端。
- 握手：服务端每条连接先发 `Challenge { nonce }`（16 字节；鉴权时 = HMAC(密钥, 种子 ‖ 连接序号) 的前 16 字节，种子 = 启动时刻的纳秒 ⊕ 8 字节系统随机数（`getrandom`）。同一次启动里连接序号不重复；重启后种子不一样，哪怕时钟一样。系统时钟早于 1970 年、拿不到系统随机数时中继不起）。带凭据的客户端等到它，回 `Hello { …, auth: { token, proof } }`，`proof` = HMAC(会话密钥, "nge-hello-v1" ‖ nonce ‖ 房间名)。不带凭据的客户端照旧连上就发 `Hello`。
- 校验顺序：长度、格式（三段、版本）→ 签名（常数时间比较）→ 解声明、查账号名字合规 → 有效期（`exp − iat`）在 1 秒到 30 天之间 → 没过期 → 从现在算也不超过 30 天（防签发时间写在未来）→ 证明。任何一步不过都回 `Unauthorized` 并断开。有效期按签发时间算：签得太长的 token 过一阵子也不会变得能用。
- 签发（`Secret::issue`）先做 `verify` 会做的检查：有效期不对、编码以后的 token 超过 512 字节（字符数没超、字节数超了，比如全是 4 字节的字）都回 `BadClaims`，签得出来的都能用。
- 为什么能防重放：抓到一次握手（token + 证明）拿到别的连接上发，随机数不一样，证明对不上；换房间也对不上；只抓到 token 没有会话密钥，算不出证明。token 过期后整个作废。
- 房间里的名字以 token 里的为准（`Hello` 里写的不算）；客户端收到 `Unauthorized` 后停止重连，换了目标（新 token）才再连。
- 密钥：中继从环境变量 `NGE_AUTH_SECRET` 读（至少 16 字节，设了但不是 UTF-8 也报错，不当没设），`Config::auth` 也可以直接给；代码和仓库里没有任何密钥，测试用测试自己写的密钥。`Secret` 的 `Debug` 只打长度；`Credential` 的 `Debug` 只打 token 的长度（token 和会话密钥都不打）；`Target`（`nge-net`）和 `NetNode`（场景的 `net`）的 `token` 字段是整串凭据，`Debug` 打成 `Some("…")`。
- 免鉴权：`nge-relay --dev` 才不鉴权；既没有 `--dev` 也没有密钥时不起，两个都有也不起，密钥不能用（太短、不是 UTF-8）时带不带 `--dev` 都不起。这个选择是 `startup::choose_auth`，四种组合都有测试。库里的 `Config::default()` 是不鉴权的（测试和嵌进别的程序用），只有命令行要求显式选。
- 签发：没有账号系统、没有登录界面，只给一个工具和一个函数：`nge-token --sub 账号 --name 名字 [--ttl 秒]`（默认 1 小时，最多 30 天）和 `Secret::issue`。谁该拿哪个账号由调用的人（自己的登录服务、运维）决定。

### 排行榜的归属

- 成绩可以走联机连接交：`Score { board, score }`，服务端记在这条连接的身份名下（鉴权时是 token 里的账号和名字，开发模式是 `Hello` 里的名字），回 `Ranked { board, rank }`。
- 一个名字第一次被哪个账号交过就归它（最多记 1 万个名字，内存里，重启清空）；别的账号、不带账号的再用这个名字交，回 `NameTaken`。比名字时先做 NFKC 规范化再转小写：`Alice`、`alice`、全角的 `Ａｌｉｃｅ` 算同一个名字。归属表满了，新名字交成绩回 `ScoresFull`（已经归了的名字照常）。
- 名字（房间名、成员名、榜名、token 里的账号和名字）不收控制字符（Cc）和看不见的格式字符（Cf：零宽空格、零宽连接符、方向控制 RLO 等、BOM、软连字符），这些能拼出看着和别人一样的名字。代价是带零宽连接符的组合 emoji（比如一家人）不能当名字。用的是 `unicode-properties` 的 Unicode 分类表（Cargo.lock 里已有的版本）。
- 一个账号最多建 8 个新榜（往已有的榜上交不限；只在内存里，重启清空）：拿到 token 的人不能用一堆新榜名把 64 个榜都挤掉。超了回 `TooManyBoards`。不带账号的开发模式不限。
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
  - 对账（`arena::client`）：任何一个字段不同都算预测错了。
  - 第二版补的：
    - 鉴权：签的时候就拒有效期不对的、编码后超过 512 字节的（4 字节的字）；长的 token 过一阵子也不能用；签发时间在未来的；过期早于签发的；密钥不是 UTF-8 报错；凭据、`Target`、`NetNode` 的 `Debug` 不打 token。
    - 启动（`startup`）：`--dev` 和密钥的四种组合、密钥不能用时带不带 `--dev` 都不起；`nge-token` 的参数（有效期夹在 1 秒到 30 天、缺参数、重复参数、不认得的参数）；随机数种子混系统随机数、时钟早于 1970 年不起。
    - 名字：零宽、RLO、BOM、软连字符这些格式字符不收；大小写、全角半角不同的同一个名字归同一个账号。
    - 排行榜：一个账号建新榜有上限、别人的榜不被挤掉；名字归属表满了回自己的错误码（`ScoresFull`），建榜太多回 `TooManyBoards`。
    - 延迟补偿：真的客户端 + 模拟网络，往返 200 ms、300 ms 拍中；回溯刚好 15 个拍中、16 个截到上限；每个输入记着按下时看到的 tick。
    - 对时：往返 3 秒不会一直从头来；往返时间取中位数，一次卡顿不算。
    - 溢出：房间和预测器的 tick 到 `u32::MAX` 停住；远端时间戳过了 50 天照样插值。
- 变异验证：见下面的表。

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

脚本逐条临时改坏，跑相应 crate 的测试，确认有测试报错，再改回原样并 touch。一共 57 条：1–28 是第一版的（随代码改了的几条跟着改了写法），29–57 是这一版补的。

- 第一版：28 条里第 23 条（对账时不比 x）第一次没抓到：模拟网络下 x 错了 vx 一般也错，补了逐个字段改的测试。
- 这一版重跑：第 3 条（从现在算的有效期上限）没抓到：有效期改成按签发时间查以后，测试里的签发时间都是「现在」，这一条只在签发时间写在未来时起作用，补了这种情况的测试；第 8 条的写法跟着代码改了（原写法编译不过，不算抓到）。其余都抓到了。

| # | 文件 | 改成 | 结果 | 报错的测试 |
|---|---|---|---|---|
| 1 | `nge-net/src/auth.rs` | 过期不拒 | 抓到 | `expired_tokens_are_refused` |
| 2 | `nge-net/src/auth.rs` | 不验签名 | 抓到 | `forged_and_tampered_tokens_are_refused` |
| 3 | `nge-net/src/auth.rs` | 从现在算的有效期不设上限 | 抓到（补了测试） | `expired_tokens_are_refused` |
| 4 | `nge-relay/src/hub.rs` | 每条连接同一个随机数（可重放） | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in` |
| 5 | `nge-relay/src/hub.rs` | 要鉴权时不带 token 也放进来 | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in` |
| 6 | `nge-relay/src/hub.rs` | 名字用 Hello 里的、不用 token 里的 | 抓到 | `with_a_secret_only_valid_signed_hellos_get_in`、`scores_sent_on_the_connection_belong_to_the_account` |
| 7 | `nge-relay/src/hub.rs` | 模式不同也能进 | 抓到 | `authority_rooms_step_and_send_snapshots` |
| 8 | `nge-relay/src/scores.rs` | 名字不归账号 | 抓到 | `a_name_belongs_to_the_first_account_that_uses_it`、`scores_sent_on_the_connection_belong_to_the_account` |
| 9 | `nge-relay/src/http.rs` | 鉴权时 HTTP 照样收分 | 抓到 | `scores_can_be_submitted_and_read` |
| 10 | `nge-net/src/arena/room.rs` | 太超前的输入不丢 | 抓到 | `inputs_are_used_by_tick_and_the_last_one_repeats` |
| 11 | `nge-net/src/arena/room.rs` | 没到的输入不沿用上一个 | 抓到 | `each_input_remembers_what_the_player_saw_when_pressing_it`、`inputs_arriving_out_of_order_are_sorted_by_tick`、`inputs_are_used_by_tick_and_the_last_one_repeats` |
| 12 | `nge-net/src/arena/room.rs` | 不做延迟补偿 | 抓到 | `tags_are_judged_where_the_attacker_saw_the_target`、`lag_compensation_covers_a_200ms_round_trip` |
| 13 | `nge-net/src/arena/room.rs` | 回溯不设上限 | 抓到 | `tags_are_judged_where_the_attacker_saw_the_target` |
| 14 | `nge-net/src/arena/room.rs` | 不按确认的基准做增量（总发全量） | 抓到 | `snapshots_are_deltas_against_what_you_acknowledged` |
| 15 | `nge-net/src/arena/delta.rs` | 没变的字段也发 | 抓到 | `a_still_world_is_a_tiny_snapshot`、`unchanged_heroes_are_not_sent_and_changed_ones_carry_only_what_changed`、`snapshots_are_deltas_against_what_you_acknowledged` |
| 16 | `nge-net/src/arena/delta.rs` | 坐标不查 NaN、越界 | 抓到 | `bad_snapshots_are_rejected` |
| 17 | `nge-net/src/arena/delta.rs` | 字段重复不查 | 抓到 | `bad_snapshots_are_rejected` |
| 18 | `nge-net/src/arena/delta.rs` | 没基准的增量当新角色 | 抓到 | `deltas_need_their_base` |
| 19 | `nge-net/src/rollback.rs` | 回滚时不重放输入 | 抓到 | `a_mismatch_rolls_back_and_replays_the_unconfirmed_inputs`、`corrections_are_smoothed_not_teleported` |
| 20 | `nge-net/src/rollback.rs` | 回滚不平滑（瞬移） | 抓到 | `corrections_are_smoothed_not_teleported` |
| 21 | `nge-net/src/rollback.rs` | 对账不比状态（从不回滚） | 抓到 | `a_long_stall_recovers`、`prediction_converges_under_delay_loss_and_reordering`、`a_huge_round_trip_does_not_reset_over_and_over` |
| 22 | `nge-net/src/arena/client.rs` | 超前量不看往返时间 | 抓到 | `lag_compensation_covers_a_200ms_round_trip`、`a_clean_steady_link_predicts_without_rollbacks` |
| 23 | `nge-net/src/arena/client.rs` | 对账不比 x | 抓到（上一版补的测试） | `any_differing_field_counts_as_a_misprediction` |
| 24 | `nge-net/src/arena/mod.rs` | 从下面也能落到平台上 | 抓到 | `runs_jumps_and_lands_back`、`standing_on_the_floor_is_stable`、`the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |
| 25 | `nge-net/src/client.rs` | 带凭据的不等随机数就发 Hello | 抓到 | `credentials_wait_for_the_challenge_and_prove_this_connection` |
| 26 | `nge-net/src/client.rs` | 鉴权不过照样重连 | 抓到 | `a_version_mismatch_or_bad_hello_stops_retrying` |
| 27 | `nge-net/src/sync.rs` | 权威房间里照样广播位置 | 抓到 | `the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |
| 28 | `nge-net/src/authority.rs` | 预测不读动作 | 抓到 | `the_own_hero_is_predicted_others_are_interpolated_and_stats_reach_tsx` |
| 29 | `nge-net/src/auth.rs` | 跳过握手证明校验 | 抓到 | `replayed_handshakes_are_refused` |
| 30 | `nge-net/src/auth.rs` | 签的时候不查有效期 | 抓到 | `issue_refuses_what_verify_would_refuse` |
| 31 | `nge-net/src/auth.rs` | 校验时不按签发时间查有效期 | 抓到 | `expired_tokens_are_refused` |
| 32 | `nge-net/src/auth.rs` | 签的时候不查 token 字节数 | 抓到 | `issue_refuses_what_verify_would_refuse` |
| 33 | `nge-net/src/auth.rs` | 密钥不是 UTF-8 当没设 | 抓到 | `secrets_from_the_environment` |
| 34 | `nge-net/src/auth.rs` | 凭据的 Debug 打出 token | 抓到 | `an_issued_credential_passes_the_handshake` |
| 35 | `nge-relay/src/startup.rs` | 密钥和 --dev 都有也起 | 抓到 | `auth_must_be_chosen_explicitly` |
| 36 | `nge-relay/src/startup.rs` | 密钥和 --dev 都没有也起（不鉴权） | 抓到 | `auth_must_be_chosen_explicitly` |
| 37 | `nge-relay/src/startup.rs` | 密钥不能用时带 --dev 照样起 | 抓到 | `auth_must_be_chosen_explicitly` |
| 38 | `nge-relay/src/startup.rs` | nge-token 的有效期不夹 | 抓到 | `token_arguments` |
| 39 | `nge-relay/src/startup.rs` | 随机数种子不混系统随机数 | 抓到 | `the_nonce_seed_mixes_in_os_randomness_and_needs_a_sane_clock` |
| 40 | `nge-relay/src/startup.rs` | 时钟早于 1970 年照样起 | 抓到 | `the_nonce_seed_mixes_in_os_randomness_and_needs_a_sane_clock` |
| 41 | `nge-net/src/proto.rs` | 名字不拒看不见的格式字符 | 抓到 | `clean_text_trims_and_rejects` |
| 42 | `nge-relay/src/scores.rs` | 名字归属不规范化（大小写、全角） | 抓到 | `a_name_belongs_to_the_first_account_that_uses_it` |
| 43 | `nge-relay/src/scores.rs` | 一个账号建新榜不限 | 抓到 | `scores_sent_on_the_connection_belong_to_the_account`、`one_account_cannot_crowd_out_every_board` |
| 44 | `nge-relay/src/scores.rs` | 名字归属表不设上限 | 抓到 | `a_full_owner_table_refuses_new_names_with_its_own_error`、`scores_sent_on_the_connection_belong_to_the_account` |
| 45 | `nge-relay/src/hub.rs` | 归属表满了回 BadFrame | 抓到 | `scores_sent_on_the_connection_belong_to_the_account` |
| 46 | `nge-relay/src/hub.rs` | 建榜太多回 BadFrame | 抓到 | `scores_sent_on_the_connection_belong_to_the_account` |
| 47 | `nge-net/src/arena/room.rs` | 回溯上限回到 9（只够 100 ms） | 抓到 | `lag_compensation_covers_a_200ms_round_trip`、`tags_are_judged_where_the_attacker_saw_the_target` |
| 48 | `nge-net/src/arena/room.rs` | 重发的输入按最新的 view 算 | 抓到 | `each_input_remembers_what_the_player_saw_when_pressing_it` |
| 49 | `nge-net/src/arena/room.rs` | 沿用的输入看到的远端不往后推 | 抓到 | `each_input_remembers_what_the_player_saw_when_pressing_it` |
| 50 | `nge-net/src/arena/room.rs` | 拍人不用按下时看到的 tick | 抓到 | `lag_compensation_covers_a_200ms_round_trip`、`tags_are_judged_where_the_attacker_saw_the_target` |
| 51 | `nge-net/src/arena/room.rs` | 房间的 tick 溢出 | 抓到 | `the_tick_stops_at_the_end_instead_of_overflowing` |
| 52 | `nge-net/src/rollback.rs` | 预测的 tick 回绕 | 抓到 | `the_tick_stops_at_the_end_instead_of_wrapping` |
| 53 | `nge-net/src/arena/client.rs` | 超前量不设上限 | 抓到 | `a_huge_round_trip_does_not_reset_over_and_over` |
| 54 | `nge-net/src/client.rs` | 往返时间只看最近一次 | 抓到 | `pongs_measure_the_round_trip` |
| 55 | `nge-net/src/client.rs` | Target 的 Debug 打出 token | 抓到 | `debug_hides_the_token` |
| 56 | `nge-scene/src/net.rs` | NetNode 的 Debug 打出 token | 抓到 | `debug_hides_the_token` |
| 57 | `nge-net/src/authority.rs` | 远端时间戳用 u32 毫秒 | 抓到 | `remote_heroes_keep_moving_after_fifty_days` |

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

第二版改的：

- 延迟补偿的回溯上限原来是 9 个 tick，实际要回溯 `ceil(往返 / DT) + 5` 个，往返超过约 100 ms 就被截（往返 133 ms 要 10 个，300 ms 要 15 个）。上限改成按要覆盖的往返（300 ms）算出来的 15 个，夹取范围改成 `[tick − 15, tick − 1]`。同时发现服务端用的是这个人最新一条消息里的 `view`，不是按拍那个 tick 时的：每个输入改成记着自己的 `view`。
- 中继要求 `--dev` 和密钥二选一的逻辑写在 `main` 里没有测试：抽成 `startup::choose_auth`。环境变量里的密钥不是 UTF-8 时原来当成「没设」（带 `--dev` 就不鉴权地起来了）：改成报错，带不带 `--dev` 都不起。
- 想要的超前量没有上限：往返 2.4 秒以上时超过预测的历史（72 个 tick），每个快照都找不到当时的预测，只能从头来（6 秒里 116 次）。夹在 52 个 tick 以内；往返时间改成最近 5 次心跳的中位数。
- `issue` 按字符数查长度，`verify` 按字节：全是 4 字节的字时能签出 571 字节、用不了的 token。`issue` 编码完按字节查。
- `issue` 不查有效期，`verify` 按 `exp − 现在` 查：31 天的 token 签得出来，过一天以后反而能用了。声明里加签发时间 `iat`，签和验都查 `exp − iat`。
- `Target`、`NetNode` 是 derive 的 `Debug`，会打出整串凭据（含会话密钥）；`Credential` 的 `Debug` 打出 token，文档却说不打内容。都改成手写，不打。
- 名字只拒控制字符，零宽空格、RLO 这些能进名字冒充别人：Cf 类一起拒；名字归属按 NFKC + 小写比。
- 握手随机数的种子只用启动时刻，时钟早于 1970 年时种子是 0，重启后序列重复：混进系统随机数，时钟早于 1970 年不起。
- 拿到 token 的账号能用 64 个新榜名把所有榜挤掉：一个账号最多建 8 个新榜。名字归属表满了时客户端收到的是 `BadFrame`：给了单独的 `ScoresFull`。
- 文档和代码对不上：「一帧最多补 8 个 tick」实际一下子补时是 16 个；「差 3 个」「多 15 个」实际是「少 4 个起」「多 16 个起」。快照里的 `ack` 客户端不读：删掉。
- 远端插值的时间戳是 `(tick × 33.3) as u32` 毫秒，约 50 天后卡在上限：改用 `f64`。房间的 `tick += 1` 在 debug 下会溢出 panic：到 `u32::MAX` 停住，客户端的 `Predictor` 一样（原来是回绕）。
- 输入队列「超过 64 个丢最新的」那段永远走不到（超前上限 60 先挡了）：删掉，换成编译期断言。
- 第二版重跑变异时第 3 条没抓到（见上表）：补了签发时间在未来的测试。

## 还没手测

下面这些只有自动测试，没有真的开窗口、真机或真的网络试过：

- 两个窗口同时 `nsh run` 进竞技场，看预测、插值、拍人、HUD 的数字（延迟、回滚、误差）在真实帧率下是否顺。
- 真实的公网延迟和丢包（测试里是模拟网络）；手机网络下的抖动。
- 要鉴权的中继配 token 在真窗口里连（集成测试里是真的中继和真的客户端，但没有渲染）；token 过期后重连的提示。
- wss：反向代理（Caddy / nginx）终止 TLS 再转给中继，没有实际配过证书试。
- iOS、Web、Android 上的竞技场：只过了编译检查，没在模拟器、浏览器、真机上跑。
- 手柄按 X 拍人、改键界面里改「拍」。
- 确定性只在同一台机器（macOS arm64）上验证过：没有在 x86_64、wasm、Android、iOS 上跑同一串输入对照结果。
- 往返 200 ms / 300 ms 的延迟补偿只在模拟网络里测过，真实网络下的手感（拍中的判定和自己看到的是否一致）没试。
