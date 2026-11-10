//! 简单的 HTTP：GET / POST，原生（ureq，每个请求一个线程）和浏览器（fetch）同一套接口。
//! `HttpClient` 管校验、并发上限和排队，每个请求一定有且只有一个 `HttpDone`。


#[cfg(feature = "ecs")]
mod ecs;
#[cfg(all(feature = "client", not(target_arch = "wasm32")))]
mod native;
#[cfg(all(feature = "client", target_arch = "wasm32"))]
mod web;

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};


#[cfg(feature = "ecs")]
pub use ecs::{HTTP, HTTP_REQUEST, Http, HttpCommand, HttpPlugin, HttpReply};

/// 同时最多几个请求在路上，多的排队。
pub const MAX_INFLIGHT: usize = 8;
/// 最多排几个：再多的马上报错。
pub const MAX_QUEUED: usize = 64;
/// 响应体最多几个字节，超出算出错。
pub const MAX_RESPONSE: u64 = 1024 * 1024;
/// 请求体最多几个字节。
pub const MAX_BODY: usize = 64 * 1024;
/// 地址最长几个字节。
pub const MAX_URL: usize = 2048;
/// 最多几个请求头。
pub const MAX_HEADERS: usize = 32;
/// 不指定时的超时（秒），最多 60。
pub const DEFAULT_TIMEOUT: f32 = 10.0;
pub const MAX_TIMEOUT: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    #[default]
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// 秒。
    pub timeout: f32,
}

impl HttpRequest {
    pub fn get(url: impl Into<String>) -> Self {
        Self { method: Method::Get, url: url.into(), headers: Vec::new(), body: None, timeout: DEFAULT_TIMEOUT }
    }

    pub fn post(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self { method: Method::Post, body: Some(body.into()), ..Self::get(url) }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn timeout(self, seconds: f32) -> Self {
        Self { timeout: seconds, ..self }
    }

    /// 只放过 http / https、合法的头、GET 不带体、超时在 (0, 60] 秒、不超长的。
    pub fn validate(&self) -> Result<(), String> {
        let lower = self.url.get(..8).unwrap_or(&self.url).to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(format!("只支持 http / https 地址: {}", self.url));
        }
        if self.url.len() > MAX_URL || self.url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err("地址太长或者有空白、控制字符".into());
        }
        if self.headers.len() > MAX_HEADERS {
            return Err(format!("请求头超过 {MAX_HEADERS} 个"));
        }
        for (name, value) in &self.headers {
            let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
            if name.is_empty() || !name.chars().all(token) {
                return Err(format!("请求头名不合规: {name:?}"));
            }
            // 只认可见的 ASCII、空格和制表符：浏览器和原生对别的字符处理不一样，干脆都拒
            if !value.chars().all(|c| c == '\t' || (' '..='~').contains(&c)) {
                return Err(format!("请求头 {name} 的值只能是 ASCII，不能有控制字符"));
            }
        }
        match (&self.method, &self.body) {
            (Method::Get, Some(_)) => return Err("GET 不能带请求体".into()),
            (_, Some(body)) if body.len() > MAX_BODY => return Err(format!("请求体超过 {MAX_BODY} 字节")),
            _ => {}
        }
        if !(self.timeout > 0.0 && self.timeout <= MAX_TIMEOUT) {
            return Err(format!("超时要在 (0, {MAX_TIMEOUT}] 秒里: {}", self.timeout));
        }
        Ok(())
    }
}

/// 响应体超过上限时的错误（原生和浏览器一样）。
#[cfg(feature = "client")]
pub(crate) fn too_large() -> String {
    format!("响应超过 {MAX_RESPONSE} 字节")
}

/// 一个请求的结果。`error` 不是空的时候是没拿到响应（地址不合规、连不上、超时、响应太大）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ecs", derive(bevy_ecs::message::Message))]
pub struct HttpDone {
    pub id: u64,
    pub status: u16,
    pub body: String,
    pub error: Option<String>,
}

impl HttpDone {
    /// 拿到了 2xx 的响应。
    pub fn ok(&self) -> bool {
        self.error.is_none() && (200..300).contains(&self.status)
    }

    pub fn failed(id: u64, error: impl Into<String>) -> Self {
        Self { id, status: 0, body: String::new(), error: Some(error.into()) }
    }
}

/// 真正发请求的：原生 / 浏览器 / 测试里的假的。
pub trait HttpBackend: Send + Sync {
    fn start(&mut self, id: u64, request: HttpRequest);
    fn poll(&mut self) -> Vec<HttpDone>;
}


/// 平台的实现。
#[cfg(feature = "client")]
pub fn backend() -> Box<dyn HttpBackend> {
    #[cfg(not(target_arch = "wasm32"))]
    return Box::new(native::NativeHttp::default());
    #[cfg(target_arch = "wasm32")]
    return Box::new(web::WebHttp::default());
}

pub struct HttpClient {
    backend: Box<dyn HttpBackend>,
    queue: VecDeque<(u64, HttpRequest)>,
    inflight: HashSet<u64>,
    next_id: u64,
    ready: Vec<HttpDone>,
}

impl HttpClient {
    pub fn new(backend: Box<dyn HttpBackend>) -> Self {
        Self { backend, queue: VecDeque::new(), inflight: HashSet::new(), next_id: 1, ready: Vec::new() }
    }

    /// 发一个请求，返回它的 id；不合规的不发，下一次 `poll` 直接给出错的结果。
    pub fn send(&mut self, request: HttpRequest) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        match request.validate() {
            Ok(()) if self.inflight.len() >= MAX_INFLIGHT && self.queue.len() >= MAX_QUEUED => {
                self.ready.push(HttpDone::failed(id, format!("排队的请求超过 {MAX_QUEUED} 个")));
            }
            Ok(()) => {
                self.queue.push_back((id, request));
                self.pump();
            }
            Err(e) => self.ready.push(HttpDone::failed(id, e)),
        }
        id
    }

    /// 取出完成了的请求，空出来的名额发排着的。
    pub fn poll(&mut self) -> Vec<HttpDone> {
        let mut done = std::mem::take(&mut self.ready);
        // 后端报了不认识的、报过的 id 不要
        done.extend(self.backend.poll().into_iter().filter(|d| self.inflight.remove(&d.id)));
        self.pump();
        done
    }

    fn pump(&mut self) {
        while self.inflight.len() < MAX_INFLIGHT
            && let Some((id, request)) = self.queue.pop_front()
        {
            self.inflight.insert(id);
            self.backend.start(id, request);
        }
    }

    /// 在路上的个数。
    pub fn inflight(&self) -> usize {
        self.inflight.len()
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 假的后端：记下开始的请求，测试往里塞结果。
    #[derive(Clone, Default)]
    pub struct FakeHttp(pub Arc<Mutex<(Vec<(u64, HttpRequest)>, Vec<HttpDone>)>>);

    impl HttpBackend for FakeHttp {
        fn start(&mut self, id: u64, request: HttpRequest) {
            self.0.lock().unwrap().0.push((id, request));
        }
        fn poll(&mut self) -> Vec<HttpDone> {
            std::mem::take(&mut self.0.lock().unwrap().1)
        }
    }

    impl FakeHttp {
        pub fn started(&self) -> Vec<(u64, HttpRequest)> {
            self.0.lock().unwrap().0.clone()
        }
        pub fn finish(&self, id: u64, status: u16, body: &str) {
            self.0.lock().unwrap().1.push(HttpDone { id, status, body: body.into(), error: None });
        }
    }

    #[test]
    fn builders_make_the_request() {
        let r = HttpRequest::get("https://x.dev/a").header("Accept", "application/json").timeout(3.0);
        assert_eq!(r, HttpRequest { method: Method::Get, url: "https://x.dev/a".into(), headers: vec![("Accept".into(), "application/json".into())], body: None, timeout: 3.0 });
        let p = HttpRequest::post("http://127.0.0.1:9100/scores", "{}");
        assert_eq!((p.method, p.body.as_deref(), p.timeout), (Method::Post, Some("{}"), DEFAULT_TIMEOUT));
        assert!(r.validate().is_ok() && p.validate().is_ok());
    }

    #[test]
    fn bad_requests_are_refused() {
        let bad = [
            HttpRequest::get("ftp://x/"),
            HttpRequest::get("javascript:alert(1)"),
            HttpRequest::get(""),
            HttpRequest::get("http://x/ a"),
            HttpRequest::get("http://x/\r\nHost: y"),
            HttpRequest::get(format!("http://x/{}", "a".repeat(MAX_URL))),
            HttpRequest::get("http://x/").header("Bad Name", "v"),
            HttpRequest::get("http://x/").header("X-A:", "v"),
            HttpRequest::get("http://x/").header("X-A", "v\r\nX-B: w"),
            HttpRequest { body: Some("x".into()), ..HttpRequest::get("http://x/") },
            HttpRequest::post("http://x/", "x".repeat(MAX_BODY + 1)),
            HttpRequest::get("http://x/").timeout(0.0),
            HttpRequest::get("http://x/").timeout(f32::NAN),
            HttpRequest::get("http://x/").timeout(MAX_TIMEOUT + 1.0),
            (0..=MAX_HEADERS).fold(HttpRequest::get("http://x/"), |r, i| r.header(format!("X-{i}"), "v")),
            // 头的值只能是 ASCII（浏览器和原生一样拒）
            HttpRequest::get("http://x/").header("X-Name", "小明"),
            HttpRequest::get("http://x/").header("X-Name", "caf\u{e9}"),
        ];
        for r in bad {
            assert!(r.validate().is_err(), "{r:?}");
        }
        assert!(HttpRequest::get("HTTPS://X/").validate().is_ok(), "协议名不分大小写");
    }

    #[test]
    fn ok_means_a_2xx_response() {
        assert!(HttpDone { id: 1, status: 204, body: String::new(), error: None }.ok());
        assert!(!HttpDone { id: 1, status: 404, body: String::new(), error: None }.ok());
        assert!(!HttpDone::failed(1, "超时").ok());
    }

    #[test]
    fn invalid_requests_fail_without_reaching_the_backend() {
        let fake = FakeHttp::default();
        let mut c = HttpClient::new(Box::new(fake.clone()));
        let id = c.send(HttpRequest::get("ftp://x/"));
        assert!(fake.started().is_empty());
        let done = c.poll();
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].id, done[0].status), (id, 0));
        assert!(done[0].error.is_some());
        assert!(c.poll().is_empty(), "只报一次");
    }

    #[test]
    fn at_most_eight_in_flight_and_the_rest_wait() {
        let fake = FakeHttp::default();
        let mut c = HttpClient::new(Box::new(fake.clone()));
        let ids: Vec<u64> = (0..10).map(|i| c.send(HttpRequest::get(format!("http://x/{i}")))).collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 10, "id 不重复");
        assert_eq!((c.inflight(), c.queued(), fake.started().len()), (MAX_INFLIGHT, 2, MAX_INFLIGHT));
        fake.finish(ids[0], 200, "a");
        let done = c.poll();
        assert_eq!(done, vec![HttpDone { id: ids[0], status: 200, body: "a".into(), error: None }]);
        assert_eq!((c.inflight(), c.queued()), (MAX_INFLIGHT, 1), "空出来的名额发了排着的");
        assert_eq!(fake.started().last().unwrap().1.url, "http://x/8", "按顺序");
    }

    #[test]
    fn the_queue_is_capped_and_overflow_fails_at_once() {
        let fake = FakeHttp::default();
        let mut c = HttpClient::new(Box::new(fake.clone()));
        let ids: Vec<u64> = (0..MAX_INFLIGHT + MAX_QUEUED + 3).map(|i| c.send(HttpRequest::get(format!("http://x/{i}")))).collect();
        assert_eq!((c.inflight(), c.queued()), (MAX_INFLIGHT, MAX_QUEUED));
        let done = c.poll();
        assert_eq!(done.iter().map(|d| d.id).collect::<Vec<_>>(), ids[MAX_INFLIGHT + MAX_QUEUED..].to_vec(), "多出来的马上报错");
        assert!(done.iter().all(|d| d.error.as_deref().is_some_and(|e| e.contains("排队"))));
    }

    #[test]
    fn each_request_completes_once_and_unknown_ids_are_ignored() {
        let fake = FakeHttp::default();
        let mut c = HttpClient::new(Box::new(fake.clone()));
        let id = c.send(HttpRequest::get("http://x/"));
        fake.finish(id, 200, "a");
        fake.finish(id, 200, "again");
        fake.finish(999, 200, "who");
        assert_eq!(c.poll().len(), 1);
        assert_eq!(c.inflight(), 0);
    }
}
