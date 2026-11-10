# 联网：WebSocket、中继、状态同步、HTTP

> 前置：TSX 场景（[m3-tsx.md](m3-tsx.md)）、平台跳跃（[platformer.md](platformer.md)）、Web（[web.md](web.md)）。

## 目标

引擎现在只能单机：两台设备上的人看不到对方，排行榜、云存档这类要请求服务器的东西也只能在 TSX 里各写各的。这一项给引擎加上最基本的联网：一条 WebSocket 连接、一个本机就能起的中继服务、基于中继的「各自广播自己、远端插值显示」的同步，以及简单的 HTTP。

**验收标准**：
- 传输：macOS / iOS（tokio-tungstenite，在单独的线程上跑）和浏览器（`WebSocket`）同一套接口；连接、断线、自动重连（指数退避加抖动）、收发二进制和文本消息；连接状态和消息进 ECS（`NetEvent` 消息）也进 TSX（`net` 事件）。不阻塞引擎线程。
- 中继：`cargo run -p nge-relay` 起一个本机服务；按房间名加入、转发、成员加入 / 离开通知、心跳超时踢出、单房间人数上限、消息大小上限、频率限制。不依赖外部服务。
- 同步：标了 `NetSync` 的实体每秒 20 次广播位置；远端实体（`Remote`）缓冲约 100 ms，按发送方的时间戳插值，乱序、丢包、重复都能容忍，等不到新数据时最多外推 100 ms 然后停住；本地实体不受影响。
- HTTP：`GET` / `POST`，原生和 web 一套接口，Rust 里是 `Http` 资源 + `HttpDone` 消息，TSX 里是返回 `Promise` 的 `http.get` / `http.post`。
- 平台跳跃：两个窗口（或一个窗口加一个浏览器）连到本机中继，同一关里能看到对方的角色（半透明、头顶名字），对方移动平滑；按 N 开关联机；连不上时照常单机，HUD 显示「离线 / 连接中 / 在线 N 人」。通关时把成绩交给中继自带的排行榜，HUD 显示这一关的第一名。
- 健壮：消息大小、频率有上限；远端来的数据都校验（NaN、无穷、越界、过长的字符串、过多的条目）；对方离开或自己断线时清掉远端实体。

## 做法

### crate

- `nge-net`：协议（`proto`）、插值缓冲（`interp`）、退避（`backoff`）、连接（`link`：原生 / web 两份实现）、客户端状态机（`client`）、HTTP（`http`），以及 ECS 插件（`plugin`，feature `ecs`，默认开）。
  - feature `client`（默认开）：连接和 HTTP 的平台实现（原生用 tokio / tokio-tungstenite / ureq，web 用 `web-sys`）。中继只用协议，关掉默认 feature。
- `nge-relay`：中继服务。房间逻辑（`hub`）是纯逻辑，时间由调用方给；`server` 用 tokio 把它接到 TCP 上；`bin/nge-relay` 读命令行参数。
- `nge-scene`：场景里的 `net` 节点（feature `net`，默认开，和 `physics3d`、`ai` 一样，长坂坡不开）。

### 消息格式

两层，都用 serde + postcard（紧凑的二进制，变长整数，枚举的序号就是消息类型）：

- 客户端 ↔ 中继（`ClientMsg` / `RelayMsg`）：`Hello { version, room, name }`、`Data { to, body }`、`Ping`；`Welcome { you, peers, max }`、`Joined`、`Left`、`Data { from, body }`、`Pong`、`Error { code }`。中继只认这一层，`body` 原样转发，不解析。
- 客户端之间（`Payload`，放在 `body` 里）：`Snapshot { seq, t, items }`（`items` 是 `{ key, x, y, rotation }`）和 `Message { kind, json }`（TSX 发的消息，数据是 JSON 文本）。

取舍：快照每秒 20 次、每人几十字节，二进制比 JSON 小一半以上、解析也快；TSX 的消息本来就是 JSON 值，放在里面原样带过去，不用为每种消息定 schema。协议带版本号（`PROTOCOL_VERSION`），对不上的中继回 `Error { code: Version }`。文本帧中继不认（回 `Error { code: BadFrame }`），传输层本身支持文本帧（`Frame::Text`）。

解码统一走 `decode_*`：先看长度上限，再用 postcard 解，最后校验（`validate`）：数不是有限的、坐标超过 ±1e6、字符串超长或有控制字符、条目超过上限的都丢掉，不让坏数据进 ECS。

### 传输（`link`）

```rust
pub enum Frame { Binary(Vec<u8>), Text(String) }
pub enum LinkEvent { Open, Frame(Frame), Closed(String) }
pub trait Link: Send + Sync {
    fn send(&mut self, frame: Frame);
    fn poll(&mut self) -> Vec<LinkEvent>;
}
pub fn connect(url: &str) -> Box<dyn Link>;
```

- 原生：每条连接一个线程，里面一个单线程的 tokio 运行时跑 `connect_async`（5 秒超时）和收发循环；和引擎之间用通道，`send` / `poll` 都不阻塞。`Link` 丢掉时通道断开，线程发 Close 后退出。单条消息上限 64 KiB（tungstenite 的 `max_message_size`）。支持 `wss://`（rustls + webpki 根证书，和 nsh 用的同一套）。
- web：`web_sys::WebSocket`，`binaryType = arraybuffer`，`onopen` / `onmessage` / `onclose` / `onerror` 往一个队列里放，引擎每帧 `poll`。浏览器上只有一个线程，`Link` 的 `Send + Sync` 是给 ECS 资源用的（同 docs/web.md 里 wgpu 的做法）。
- 引擎这边每帧调一次 `poll`，没有任何阻塞调用。

### 客户端（`NetClient`）

纯逻辑的状态机，连接由注入的 `connector` 建（测试里换成假的）：

- `set_target(Some(Target { url, room, name }))` 开始连，`None` 断开；目标变了（换房间）先断再连。
- 状态 `Offline` / `Connecting` / `Online`。连上后发 `Hello`，收到 `Welcome` 才算 `Online`。
- 断线（对方关了、出错、10 秒没收到任何东西）：所有远端成员发 `Left`，转 `Connecting`，按退避等一会儿再连。
- 退避（`Backoff`）：0.5 秒起，每次翻倍，最多 10 秒，±20% 抖动（抖动的随机数可以注入，测试里固定）；`Welcome` 之后归零。
- 心跳：每 2 秒发一次 `Ping`；中继回 `Pong`，也算收到东西。
- 发出去的用户消息每秒最多 30 条（令牌桶），超出的丢掉并返回 `false`；快照有自己的频率，不算在里面。

### 中继（`nge-relay`）

- `Hub`（纯逻辑）：`connect` / `frame` / `close` / `tick(now)` 返回要发的消息和要断的连接。
  - 第一条必须是 `Hello`，5 秒内不发就断；版本不对、房间名或名字不合规（空、超长、控制字符）回 `Error` 后断。
  - 房间满了（默认 8 人）回 `Error { code: RoomFull }` 后断。
  - 进房间回 `Welcome`（自己的 id、现有成员），其他人收到 `Joined`；离开（关闭、超时、被踢）时其他人收到 `Left`；空房间删掉。
  - `Data` 转给房间里的其他人（`to` 指定时只给那一个，不在这个房间的忽略）。
  - 超过 `max_frame`（默认 8 KiB）的帧回 `Error { code: TooLarge }`，丢掉；每条连接一个令牌桶（默认每秒 60、攒 120），超出的丢掉，连续超出很多（默认 600 条）就断开。
  - 10 秒没收到任何帧就踢出（客户端 2 秒一次心跳）。
- `server::serve(listener, config)`：每条连接一个任务读写，`Hub` 在一个任务里，按 1 秒一次 `tick`。
- 顺带一个很小的排行榜（HTTP，同一个端口）：`GET /scores?board=<名字>` 返回前 10 名，`POST /scores` 交 `{ board, name, score }`（分数小的在前，比如通关秒数）。只在内存里，每个榜最多 100 条、最多 64 个榜，请求体最多 4 KiB。带 `Access-Control-Allow-Origin: *`，浏览器里也能调。只是为了演示 HTTP 不依赖外部服务。
- `cargo run -p nge-relay -- --addr 127.0.0.1:9100 --room-size 8`。

### ECS（`NetPlugin`）

- 资源 `Net`：客户端、本机的时钟、对每个远端的插值缓冲。`Net::connect(target)` / `Net::disconnect()` / `Net::send(kind, json, to)` / `Net::status()` / `Net::peers()`。
- 组件：
  - `NetSync { key }`：每秒 `rate`（默认 20）次把它的 `Transform2d`（位置、角度）放进快照广播。
  - `Remote { peer, key }`：按收到的快照插值，写它的 `Transform2d`。
  - `KeepOnLeave`：场景声明的远端实体带它，对方离开时不在 Rust 这边删，交给场景（TSX 会把它从场景里去掉）。没有它的 `Remote` 实体在对方离开、自己断线时被删掉。
- 消息 `NetEvent`：`Status`、`Joined`、`Left`、`Message`、`Error`。同时发给 TSX 的 `net` 事件（`kind` 是 `status` / `join` / `leave` / `message` / `error`）；`status` 带着在连的中继地址 `url`（断开时是 null）和成员列表。
- TSX 的 command：`netSend { type, data, to? }`、`http { id, method, url, headers, body, timeout }`；事件：`net`、`http { id, ok, status, body, error }`。
- HTTP 是单独的 `HttpPlugin`（`Http` 资源、`HttpDone` 消息、`http` command 和事件）；场景开了 feature `net` 时两个插件都加上。
- 系统：`PreUpdate` 收（更新状态、喂缓冲、发事件），`PostUpdate`（物理之后、相机之前）给 `Remote` 写插值后的位置、到点了广播快照。

### 插值（`InterpBuffer`）

- 每个远端每个 `key` 一个缓冲，按发送方时间戳 `t`（毫秒）排好，最多 32 个；同一个 `seq` 的重复包、比已经播过的还旧的包丢掉，乱序的插到该在的位置。
- 时钟：每收到一个包算 `本地时间 − t`，取最小的那次（路上最快的那个）当偏移，之后按 2% 的速度往上跟（应对两边时钟走得不一样快）。
- 一下跳得太远（超过 300，比如对方重生回起点）直接跳过去，不在中间拖一条线。要画的时间 = 本地时间 − 偏移 − 延迟（默认 100 ms）。
- 要画的时间落在两个包之间：按时间线性插值位置，角度走短的那边。比最新的还新：按最后两个包的速度外推，最多 100 ms，之后停在那里。比最旧的还旧：用最旧的。
- 角度、位置不是有限的数时整包丢掉（解码时已经校验过一次）。

### HTTP

```rust
pub struct HttpRequest { pub method: Method, pub url: String, pub headers: Vec<(String, String)>, pub body: Option<String>, pub timeout: f32 }
pub struct HttpDone { pub id: u64, pub status: u16, pub body: String, pub error: Option<String> }
```

- `HttpRequest::get(url)` / `post(url, body)`，`header`、`timeout` 链式加（默认 10 秒）；`validate` 只放过 `http://` / `https://`、地址不超过 2048 字节且没有空白和控制字符、最多 32 个头且头名合法、`GET` 不带体、请求体不超过 64 KiB、超时在 (0, 60] 秒。不合规的直接回一个带 `error` 的 `HttpDone`，不发出去。
- `Http` 资源：`send(req) -> id`；每帧收已经完成的请求，变成 `HttpDone` 消息，TSX 发的再变成 `http` 事件（带 TSX 自己的 id）。同时最多 8 个在路上，多的排队。响应体最多 1 MiB，超出算出错。每个请求一定有且只有一个 `HttpDone`。
- 原生：每个请求一个线程跑 `ureq`（阻塞调用在那个线程里，不在引擎线程上）。web：`fetch`（`wasm_bindgen_futures::spawn_local`），超时用 `AbortController`。
- TSX（`ts/http.ts`）：`const http = httpClient(() => game)`，`<GameView onHttp={http.handle}>`，`await http.get(url, { headers, timeout })`、`await http.post(url, body)`，结果是 `{ ok, status, body, error? }`；`post` 的 `body` 不是字符串时编成 JSON，默认 `Content-Type: text/plain`（浏览器跨域不用预检）；游戏视图还没建好时直接以 `error` 结束；`json(result)` 只解 2xx 的合法 JSON。

### 场景和 TSX

```ts
scene.net = { url?: string, room: string, name?: string, sync?: string[], rate?: number, delay?: number }
```

- 写了 `net` 就连，去掉就断，`url` / `room` / `name` 变了重连。`url` 不写时用环境变量 `NGE_RELAY`（原生），再没有就是 `ws://127.0.0.1:9100`。
- `sync` 里的精灵 id 带上 `NetSync { key: id }`。
- id 是 `net:<peer>:<key>` 的精灵（`remoteId(peer, key)`）带上 `Remote` 和 `KeepOnLeave`：TSX 按成员列表声明它们长什么样（颜色、大小、头顶的名字是 `bubbles` 里 `follow` 它的气泡），位置归插值管。场景没变的节点不动，插值写的位置不会被场景盖掉。
- `netRoom()`：从 `net` 事件维护 `status()`、`url()`、`me()`、`peers()`、`error()` 的 signal，`onMessage(type, fn)` 订阅消息（不在房间里的人发的不要）；`netSend(game, type, data, to?)` 发消息。断线时 `peers()` 清空，远端的精灵跟着从场景里去掉。
- `relayHttp(url)`：中继地址换成同一台服务器的 HTTP 地址（`ws://h:p/…` → `http://h:p`，`wss` → `https`），排行榜用它。

### 平台跳跃

- 联网默认开着，按 N 开关（`saved("net")`，记在设备上）；房间是 `platformer-<关卡号>`：同一关的人互相看得见，换关就换房间。名字是随机的 `P100`–`P999`。
- 远端的角色是半透明的蓝色方块（`#5dade280`），头顶是对方的名字，没有刚体（碰不到、不会被踩）。
- HUD 加一段：关着时「离线」，连着（包括中继连不上、一直在重试）时「连接中…」，连上了「在线 N 人」（算上自己）。
- 通关时 `POST` 成绩到中继的排行榜，每关开始、连上中继、交完成绩后 `GET` 一次第一名显示在 HUD 上（「榜首 P123 12.3 秒」）；排行榜的地址从 `net` 事件里的中继地址推出来；中继没起来时不显示，不影响游戏。对面给的榜单只认类型对、分数是有限非负数的，名字截到 32 个字。
- 代码在 `examples/platformer/online.ts`（纯函数，`online.test.tsx` 测）和 `App.tsx`。

## 不做

- 权威服务器、预测和回滚、延迟补偿；对方的角色只是显示，不参与碰撞和踩敌人。
- 断线期间的本地缓冲重发、可靠有序的消息通道（WebSocket 本身就是可靠有序的，断了就丢）。
- 加密、鉴权、防作弊；排行榜谁都能交分。
- 中继的多进程 / 多机扩展、持久化。
- NAT 穿透、P2P、WebRTC。
- iOS 后台保持连接（切到后台系统会断，回来自动重连）。

## 验收

- `cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、平台跳跃和长坂坡逐个 `nsh test`、`nsh check`、iOS 的 `cargo check`、`tools/check-wasm.sh` 通过。
- 单元测试：编解码（往返、长度上限、坏数据、NaN、越界、超长字符串）、插值（顺序、乱序、重复、丢包、外推上限、时钟偏移）、退避（翻倍、上限、抖动范围、归零）、客户端（握手、断线发 Left、重连按退避、心跳超时、换房间、发送频率）、房间（加入、离开通知、人数上限、心跳超时、大小上限、频率限制、Hello 前发数据）、HTTP（请求构造和校验、并发上限排队、回调按 id 分发、不合规的立即回错）。
- 集成测试：测试里起中继（随机端口），两个原生客户端连同一房间，互相收到快照和离开通知；原生连接收发文本和二进制帧；排行榜的 HTTP。
- 变异验证：见下面的表。

### 本机试联网

```sh
cargo run -p nge-relay                      # 起中继，默认 127.0.0.1:9100
cd examples/platformer && nsh run           # 开两个（第二个在另一个终端里再 nsh run）
NGE_RELAY=ws://192.168.1.10:9100 nsh run    # 中继在别的机器上（原生读环境变量）
curl 'http://127.0.0.1:9100/scores?board=platformer-1'   # 看排行榜
```

两个窗口在同一关时互相看得见；关掉中继后 HUD 变成「连接中…」，照样能玩，中继再起来后自动连回去。

### 变异验证

临时改坏关键逻辑，确认有测试报错，再改回来：

| 改坏的地方 | 报错的测试 |
|---|---|
| 插值不丢重复的 `seq` | `interp::duplicates_and_already_played_samples_are_dropped` |
| 外推不封顶 | `interp::extrapolates_at_most_the_cap_then_holds`、`sync::remotes_are_interpolated_a_hundred_ms_behind` |
| 跳得远也插值（不瞬移） | `interp::teleports_are_not_smeared` |
| 退避不封顶 | `backoff::jitter_stays_within_twenty_percent_and_varies` 等 3 个 |
| 断线不给成员发 `Left` | `client::a_dropped_connection_reports_everyone_left_and_reconnects_with_backoff` 等 2 个 |
| 不判 10 秒静默 | `client::pings_every_two_seconds_and_gives_up_after_ten_silent_seconds` 等 2 个 |
| 房间不限人数 | `hub::a_full_room_turns_you_away` |
| 不限帧大小 | `hub::oversized_frames_are_refused_but_the_connection_stays` |
| 刷屏不踢 | `hub::floods_are_dropped_and_a_long_flood_is_kicked` |
| 心跳超时不踢 | `hub::silent_members_and_never_joining_connections_time_out` |
| 快照不查 NaN / 不查越界 | `proto::non_finite_and_out_of_range_coordinates_are_rejected` |
| HTTP 不限并发 | `http::at_most_eight_in_flight_and_the_rest_wait` |
| HTTP 不查协议 | `http::bad_requests_are_refused` 等 3 个 |
| 忽略 `KeepOnLeave` / 对方走了不删 | `sync::remotes_of_a_peer_who_left_are_removed_unless_the_scene_keeps_them`、`sync::going_offline_removes_remotes` |
| TSX：结果交回后不删 id | `http.test.tsx` 的「结果按 id 交回……」 |
| TSX：`status` 里的成员不过滤自己、不在线也算 | `net.test.tsx` 的「status 里的成员只在在线时算，自己不算」（原来的测试没抓到，补的） |
| TSX：榜首不查名字类型 | `online.test.tsx` 的「榜首：……不信对面的数据」 |

## 修过的问题

- 排行榜的请求体太大时，中继没读完请求就回 413、关连接，客户端收到的是连接被重置：改成先读完请求头再回。
- 百分号解码按字符串切片，碰到多字节字符会 panic：改成按字节解析十六进制。
- 场景写了 `net` 的那一帧还没连：连接的系统排到收消息之前。

## 还没手测

- 两个窗口 / 一个窗口加浏览器连本机中继实际看对方的角色（没有开游戏窗口）。
- iOS 真机连电脑上的中继（要把 `url` 改成电脑的局域网地址）。
- 浏览器里的 WebSocket 和 fetch（只编过 wasm，没在浏览器里跑）。
