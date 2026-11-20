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
- 状态 `Offline` / `Connecting` / `Online` / `Full`。连上后发 `Hello`，收到 `Welcome` 才算 `Online`。
- 名字和房间名先在本地按中继的规则校验（去掉首尾空白、截长、不能有控制字符），不合规的不连，直接 `Offline` 并报错。
- 中继回 `Version`（协议版本不对）或 `BadHello`：重连也没用，停下来转 `Offline` 并报错，直到目标再变。
- 中继回 `RoomFull`：转 `Full`（TSX 里是 `"full"`，和一直连不上分得开），30 秒后再试。
- 断线（对方关了、出错、10 秒没收到任何东西）：所有远端成员发 `Left`，转 `Connecting`，按退避等一会儿再连。
- 退避（`Backoff`）：0.5 秒起，每次翻倍，最多 10 秒，±20% 抖动（抖动的随机数可以注入，测试里固定）；`Welcome` 之后归零。
- 心跳：每 2 秒发一次 `Ping`；中继回 `Pong`，也算收到东西。
- 发出去的用户消息每秒最多 30 条（令牌桶），超出的丢掉，`send_message` / `Net::send` 返回 `Err(SendError::RateLimited)`（没在线是 `NotOnline`，种类或内容不合规是 `Invalid`）；快照有自己的频率，不算在里面。
- 和中继的限流对得上：中继每条连接每秒 60 帧，客户端最多 30 条消息 + 25 个快照 + 0.5 个心跳，正常用不会被中继丢。

### 中继（`nge-relay`）

- `Hub`（纯逻辑）：`connect` / `frame` / `close` / `tick(now)` 返回要发的消息和要断的连接。
  - 第一条必须是 `Hello`，5 秒内不发就断；版本不对、房间名或名字不合规（空、超长、控制字符）回 `Error` 后断。
  - 同一个房间里重名的，后来的自动加编号（`a#2`、`a#3`……，加完还是不超过名字的长度上限）。
  - 房间满了（默认 8 人）回 `Error { code: RoomFull }` 后断。
  - 进房间回 `Welcome`（自己的 id、现有成员），其他人收到 `Joined`；离开（关闭、超时、被踢）时其他人收到 `Left`；空房间删掉。
  - `Data` 转给房间里的其他人（`to` 指定时只给那一个，不在这个房间的忽略）。
  - 超过 `max_frame`（默认 8 KiB）的帧回 `Error { code: TooLarge }`，丢掉；每条连接一个令牌桶（默认每秒 60、攒 120），二进制帧和文本帧都算，超出的丢掉，连续超出很多（默认 600 条）就断开。
  - 10 秒没收到任何帧就踢出（客户端 2 秒一次心跳）。
- `server::serve(listener, config)`（测试里用 `serve_on(acceptor, config, live)` 换掉 accept、数连接；`spawn(addr, config)` 在后台线程起一个）：
  - `Hub` 在一个任务里，按 1 秒一次 `tick`；各连接发给它的事件走一个有上限（4096）的通道。
  - 每条连接一个任务，读和写在里面并行（`select!`），任何一边结束连接就结束，结束时归还连接 id 和名额。
  - accept 出错：监听的 socket 本身坏了（`EBADF` / `EINVAL` / `ENOTSOCK`）才退出；其他（文件描述符用完、对方中途断了）记一行日志，歇 100 ms 接着 accept。
  - 同时的连接数有上限（`--max-conns`，默认 1024），满了新来的直接关掉。
  - 每条连接待发的帧有上限（`outbox`，默认 256）：排满了说明对方不读，当慢客户端踢掉，不拖住房间里的其他人；写一帧、关连接都有 5 秒超时。踢人时关闭帧排在已经待发的后面，排不进去就直接断。
  - 连接 id 用到头回绕时跳过还在用的。
- 顺带一个很小的排行榜（HTTP，同一个端口）：`GET /scores?board=<名字>` 返回前 10 名，`POST /scores` 交 `{ board, name, score }`（分数小的在前，比如通关秒数）。只在内存里，每个榜最多 100 条、最多 64 个榜，请求体最多 4 KiB；一个名字在一个榜里只留一条，成绩更好才替换；榜满了再来新榜时去掉最久没写过的那个。请求体太大时先读掉最多 64 KiB（最多等 1 秒）再回 413，客户端能收到响应而不是连接被重置。带 `Access-Control-Allow-Origin: *`，浏览器里也能调。只是为了演示 HTTP 不依赖外部服务。
- `cargo run -p nge-relay -- --dev --addr 127.0.0.1:9100 --room-size 8 --max-conns 1024`（`--dev` 不鉴权；要鉴权时设 `NGE_AUTH_SECRET`，见 [net-more.md](net-more.md)）。

### ECS（`NetPlugin`）

- 资源 `Net`：客户端、本机的时钟、快照频率 `rate`（默认 20，最多 25，不是正的有限数时按 20）、远端延迟 `delay`（毫秒，默认 100）。`Net::connect(target)` / `Net::disconnect()` / `Net::send(kind, data, to) -> Result<(), SendError>` / `Net::status()` / `Net::me()` / `Net::peers()`。
- 资源 `Remotes`：对每个远端、每个 `key` 的插值缓冲。
- 组件：
  - `NetSync { key }`：每秒 `rate` 次把它的 `Transform2d`（位置、角度）放进快照广播。
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
- 防伪造的时间戳：同一个远端的包，`t` 比上一个包往前跳得比本地过去的时间还多 5 秒以上（`MAX_JUMP`）的丢掉，不然一个 `t = u32::MAX` 的包会让时钟偏移卡死、之后的包全被当成太旧；`t` 往回拨 1 秒以上（`MAX_REWIND`，对方重启了）的把这个远端的缓冲清掉从头来。

### HTTP

```rust
pub struct HttpRequest { pub method: Method, pub url: String, pub headers: Vec<(String, String)>, pub body: Option<String>, pub timeout: f32 }
pub struct HttpDone { pub id: u64, pub status: u16, pub body: String, pub error: Option<String> }
```

- `HttpRequest::get(url)` / `post(url, body)`，`header`、`timeout` 链式加（默认 10 秒）；`validate` 只放过 `http://` / `https://`、地址不超过 2048 字节且没有空白和控制字符、最多 32 个头且头名合法、`GET` 不带体、请求体不超过 64 KiB、超时在 (0, 60] 秒。不合规的直接回一个带 `error` 的 `HttpDone`，不发出去。
- `Http` 资源：`send(req) -> id`；每帧收已经完成的请求，变成 `HttpDone` 消息，TSX 发的再变成 `http` 事件（带 TSX 自己的 id）。同时最多 8 个在路上，多的排队，排队最多 64 个，再多的立即回错。响应体最多 1 MiB，超出算出错（原生和浏览器是同一句错误）。每个请求一定有且只有一个 `HttpDone`。请求头的值只认可见 ASCII、空格和制表符（浏览器的 `Headers` 对别的字符会直接抛错）。
- 原生：每个请求一个线程跑 `ureq`（阻塞调用在那个线程里，不在引擎线程上），按字节读响应，不是 UTF-8 的用替换字符解码。web：`fetch`（`wasm_bindgen_futures::spawn_local`），超时用 `AbortController`，定时器在请求结束时清掉、回调随之释放；先看 `Content-Length`，超限的不读体。
- TSX（`ts/http.ts`）：`const http = httpClient(() => game)`，`<GameView onHttp={http.handle}>`，`await http.get(url, { headers, timeout })`、`await http.post(url, body)`，结果是 `{ ok, status, body, error? }`；`post` 的 `body` 不是字符串时编成 JSON，默认 `Content-Type: text/plain`（浏览器跨域不用预检）；游戏视图还没建好时直接以 `error` 结束；引擎一直没回（command 丢了）时，到 `timeout` 再加 2 秒（`httpClient(game, { graceMs })`）以「引擎没有回应」结束，晚到的结果不再交；自己写的头不管大小写都盖掉默认的 `Content-Type`，不会重复；`json(result)` 只解 2xx 的合法 JSON。

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

- 联网默认开着，按 N 开关（`netSetting()`，记在设备上，键 `nge:net`）；开关从存储读出来之前场景里不写 `net`，读到的不是布尔值按开着算；房间是 `platformer-<关卡号>`：同一关的人互相看得见，换关就换房间。名字是随机的 `P100`–`P999`。
- 远端的角色是半透明的蓝色方块（`#5dade280`），头顶是对方的名字，没有刚体（碰不到、不会被踩）。
- HUD 加一段：关着（包括开关还没读出来）时「离线」，连着（包括中继连不上、一直在重试）时「连接中…」，房间满了「房间满了，稍后再试」，连上了「在线 N 人」（算上自己）。
- 通关时 `POST` 成绩到中继的排行榜，每关开始、连上中继、交完成绩后 `GET` 一次第一名显示在 HUD 上（「榜首 P123 12.3 秒」）；排行榜的地址从 `net` 事件里的中继地址推出来；中继没起来时不显示，不影响游戏。对面给的榜单只认类型对、分数是有限非负数的，名字截到 32 个字。
- 代码在 `examples/platformer/online.ts`（纯函数，`online.test.tsx` 测）和 `game.tsx`。

## 不做

- ~~权威服务器、预测和回滚、延迟补偿~~：已做，见 [net-more.md](net-more.md)（平台跳跃按 M 进的权威竞技场）；主场景的中继联机里对方的角色仍然只是显示，不参与碰撞和踩敌人。
- 断线期间的本地缓冲重发、可靠有序的消息通道（WebSocket 本身就是可靠有序的，断了就丢）。
- ~~鉴权~~：已做，见 [net-more.md](net-more.md)（HMAC token、排行榜按账号归属，`--dev` 才免鉴权）；~~加密~~：走 TLS 反向代理加 `wss://`，见 [net-more.md](net-more.md)；防作弊仍不做。免鉴权的开发模式下排行榜照旧谁都能交分。
- 排行榜的淘汰策略之外的配额：每个来源能建几个榜、交几次分不限（榜满了只是挤掉最久没写的那个）。
- 中继的多进程 / 多机扩展、持久化。
- NAT 穿透、P2P、WebRTC。
- iOS 后台保持连接（切到后台系统会断，回来自动重连）。

## 验收

- `cargo test --workspace`、`cargo test -p nge-scene --no-default-features`、平台跳跃和长坂坡逐个 `nsh test`、`nsh check`、iOS 的 `cargo check`、`tools/check-wasm.sh` 通过。
- 单元测试：编解码（往返、长度上限、坏数据、NaN、越界、超长字符串）、插值（顺序、乱序、重复、丢包、外推上限、时钟偏移）、退避（翻倍、上限、抖动范围、归零）、客户端（握手、断线发 Left、重连按退避、心跳超时、换房间、发送频率）、房间（加入、离开通知、人数上限、心跳超时、大小上限、频率限制、Hello 前发数据）、HTTP（请求构造和校验、并发上限排队、回调按 id 分发、不合规的立即回错）。
- 集成测试：测试里起中继（随机端口），两个原生客户端连同一房间，互相收到快照和离开通知；原生连接收发文本和二进制帧；排行榜的 HTTP。
- 中继本身（`nge-relay/tests/server.rs`）：accept 暂时出错照常服务、监听坏了退出、超过连接数上限的被关掉、只连不读的客户端被踢而其他人照常收发、被踢的连接任务真的退出、过大的请求体照样收到 413。
- 原生连接（`nge-net/tests/link_threads.rs`）：丢掉 `Link` 后线程退出、对方不读时 2 秒内报断开且线程退出、写不出去时按超时报断开。
- 原生 HTTP（`nge-net/tests/http_native.rs`）：非 UTF-8 的响应按替换字符解码、超过 1 MiB 的响应报错。
- 变异验证：见下面的表。

### 本机试联网

```sh
cargo run -p nge-relay -- --dev             # 起中继（本机开发不鉴权），默认 127.0.0.1:9100
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
| accept 出错就退出 | `server::transient_accept_errors_do_not_stop_the_relay` |
| 不限连接数 | `server::connections_over_the_limit_are_closed_and_the_relay_lives_on` |
| 待发排满不踢 / 踢人时不断开 | `server::a_client_that_never_reads_is_kicked_and_the_others_carry_on` |
| 413 前不读掉请求体 | `server::an_oversized_http_body_still_gets_a_413` |
| 连接 id 回绕不跳过在用的 | `server::ids_wrap_around_and_skip_the_ones_in_use` |
| 文本帧不限流 | `hub::text_frames_share_the_rate_limit_and_a_text_flood_is_kicked` |
| 重名不加编号 | `hub::same_names_in_a_room_get_a_number` |
| 排行榜同名不合并 | `scores::one_entry_per_name_and_only_a_better_score_replaces_it` |
| 榜满了不淘汰 / 淘汰最新写的 | `scores::a_full_set_of_boards_drops_the_one_written_longest_ago` |
| 原生连接待发排满不当断开 | `link_threads::a_peer_that_never_reads_cannot_hang_the_link` |
| 原生连接写不限时 | `link_threads::a_write_that_never_finishes_times_out` |
| `Link` 丢掉后不限时关 | `link_threads::dropping_a_link_ends_its_thread` |
| 版本不对 / BadHello 照样重连 | `client::a_version_mismatch_or_bad_hello_stops_retrying` |
| 房间满了按普通退避重连 | `client::a_full_room_waits_long_before_trying_again` |
| 名字不在本地校验 | `client::bad_names_and_rooms_fail_locally_without_connecting` |
| 快照频率不封顶 / NaN 不回落 | `sync::the_snapshot_rate_is_capped_and_bad_rates_fall_back` |
| 不丢往前跳太多的包 / 不处理往回拨 | `sync::a_forged_timestamp_cannot_freeze_a_remote` |
| HTTP 排队不限 | `http::the_queue_is_capped_and_overflow_fails_at_once` |
| 请求头的值不查 ASCII | `http::bad_requests_are_refused` |
| 原生响应遇到非 UTF-8 报错 / 超限不报同样的错 | `http_native::a_body_that_is_not_utf8_is_decoded_lossily`、`http_native::a_response_over_the_limit_is_an_error` |
| TSX：默认头不按小写去重 | `http.test.tsx` 的「自己写的 content-type 不管大小写都盖掉默认的……」 |
| TSX：没有兜底超时 | `http.test.tsx` 的「引擎一直没回……」 |
| TSX：开关不查布尔值 / 没读完就写 `net` | `online.test.tsx` 的「联网开关：……」 |
| TSX：房间满了不单独显示 | `online.test.tsx` 的「HUD：房间满了单独显示……」 |

等价的改法（改了测试也抓不到，行为确实没变）：踢人时关闭帧不排进待发队列、直接丢掉发送端——写的任务在通道关了时一样会结束，连接照样断开。

## 修过的问题

- 排行榜的请求体太大时，中继没读完请求就回 413、关连接，客户端收到的是连接被重置：改成先读完请求头再回。
- 百分号解码按字符串切片，碰到多字节字符会 panic：改成按字节解析十六进制。
- 场景写了 `net` 的那一帧还没连：连接的系统排到收消息之前。
- 中继 accept 出一次错（比如文件描述符用完）整个服务就退出：改成只有监听的 socket 坏了才退出，其他记日志、歇一下接着 accept；同时加了连接数上限。
- 有人只连不读，中继给他的待发帧无限堆积，写的时候还可能一直卡着：待发有上限，排满了踢掉；写和关都有超时；`Hub` 的收件通道也有上限。
- 文本帧不走限流，可以拿来刷屏：和二进制帧共用令牌桶。
- 原生连接的读和写在一个循环里，对方不读时 `send` 会把读也卡住，丢掉 `Link` 后线程可能一直不退：读写分开并行，待发有上限，写和关都有超时。
- 协议版本不对、Hello 不合规时客户端一直重连；房间满了和连不上分不开：前者停下转离线，房间满了单独一个状态、30 秒后再试；名字和房间名先在本地校验。
- `rate` 设得很大会超过中继的限流被踢，设成 NaN 时不发快照：最多 25，不合规的按 20。
- 一个时间戳是 `u32::MAX` 的包能让远端从此停住：往前跳得离谱的包丢掉，往回拨的从头来。
- 连接 id 回绕后可能和还在用的撞上：跳过在用的。
- 排行榜一个人刷很多条就能占满前十；榜满了以后新榜永远建不了：一个名字只留最好的一条；榜满了挤掉最久没写的。
- 浏览器的 HTTP 超时定时器的回调每个请求漏一个；不看 `Content-Length` 先把整个体读进来；请求头的值有非 ASCII 字符时浏览器直接抛错：都改了。
- 原生 HTTP 收到非 UTF-8 的响应报错：改成替换字符解码。
- HTTP 排队不限长；TSX 那边引擎一直不回时 `Promise` 永远不结束；默认头和自己写的大小写不同时会重复：排队最多 64 个，TSX 加兜底超时，头按小写去重。
- 平台跳跃开关还没从存储读出来时先按开着连了一次；存的不是布尔值时行为不定：读出来之前不写 `net`，不是布尔值按开着。
- 同房间两个人同名，头顶名字分不出来：后来的加编号。

## 还没手测

- 两个窗口 / 一个窗口加浏览器连本机中继实际看对方的角色（没有开游戏窗口）。
- iOS 真机连电脑上的中继（要把 `url` 改成电脑的局域网地址）。
- 浏览器冒烟跑过一次：本机中继加上平台跳跃的 web 包（临时注册了 web 上的游戏组件，没提交），WebSocket 连上、收到中继的消息，HUD 显示「在线 1 人」；`fetch` 拿到排行榜，HUD 显示「榜首 Smoke 12.3 秒」；按 N 关掉、再打开能重新连上。浏览器窗口在后台时引擎不跑帧，状态会停在「连接中…」，回到前台才继续。
- 平台跳跃里按 N、HUD 各个状态的切换没有在 `nsh test` 里测（测试环境没有游戏组件，发不出按键和 `net` 事件），只测了 `online.ts` 里的纯函数。
