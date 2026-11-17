//! HTTP 的 ECS 部分：`Http` 资源（Rust 发请求）、`HttpDone` 消息；TSX 的 `http` command 发、`http` 事件收（带 TSX 自己的 id）。

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use bevy_ecs::prelude::*;
use nge_core::{CommandKey, EventKey};
use nge_ecs::{App, Outbox, Plugin, PreUpdate};
use serde::{Deserialize, Serialize};

use super::{DEFAULT_TIMEOUT, HttpBackend, HttpClient, HttpDone, HttpRequest, Method};

/// 发给 TSX 的结果。
pub const HTTP: EventKey<HttpReply> = EventKey::new("http");
/// TSX 发请求。
pub const HTTP_REQUEST: CommandKey<HttpCommand> = CommandKey::new("http");

#[derive(Message, Debug, Clone, PartialEq, Deserialize)]
pub struct HttpCommand {
    /// TSX 自己的 id，结果原样带回去。
    pub id: u64,
    #[serde(default)]
    pub method: Method,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    /// 秒。
    #[serde(default)]
    pub timeout: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HttpReply {
    pub id: u64,
    pub ok: bool,
    pub status: u16,
    pub body: String,
    pub error: Option<String>,
}

#[derive(Resource)]
pub struct Http {
    client: HttpClient,
    /// 我们的 id → TSX 的 id。
    from_tsx: HashMap<u64, u64>,
}

impl Http {
    /// 发一个请求，结果是同一个 id 的 `HttpDone` 消息。
    pub fn send(&mut self, request: HttpRequest) -> u64 {
        self.client.send(request)
    }

    pub fn inflight(&self) -> usize {
        self.client.inflight()
    }
}

/// 加上 `Http` 资源、`HttpDone` 消息和 TSX 的 `http` 通道。
pub struct HttpPlugin {
    backend: Mutex<Option<Box<dyn HttpBackend>>>,
}

impl HttpPlugin {
    pub fn new() -> Self {
        Self { backend: Mutex::new(None) }
    }

    /// 换掉真正发请求的（测试用假的）。
    pub fn backend(self, backend: Box<dyn HttpBackend>) -> Self {
        *self.backend.lock().unwrap_or_else(|e| e.into_inner()) = Some(backend);
        self
    }
}

impl Default for HttpPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for HttpPlugin {
    fn build(&self, app: &mut App) {
        let backend = self.backend.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(super::backend);
        app.insert_resource(Http { client: HttpClient::new(backend), from_tsx: HashMap::new() })
            .add_message::<HttpDone>()
            .add_event(HTTP)
            .add_command(HTTP_REQUEST)
            .add_systems(PreUpdate, (requests, finish).chain());
    }
}

fn requests(mut http: ResMut<Http>, mut commands: MessageReader<HttpCommand>) {
    for cmd in commands.read() {
        let request = HttpRequest {
            method: cmd.method,
            url: cmd.url.clone(),
            headers: cmd.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            body: cmd.body.clone(),
            timeout: cmd.timeout.unwrap_or(DEFAULT_TIMEOUT),
        };
        let id = http.client.send(request);
        http.from_tsx.insert(id, cmd.id);
    }
}

fn finish(mut http: ResMut<Http>, mut done: MessageWriter<HttpDone>, mut outbox: ResMut<Outbox>) {
    for d in http.client.poll() {
        if let Some(id) = http.from_tsx.remove(&d.id) {
            outbox.send(HTTP, &HttpReply { id, ok: d.ok(), status: d.status, body: d.body.clone(), error: d.error.clone() });
        }
        done.write(d);
    }
}

#[cfg(test)]
mod tests {
    use nge_core::Viewport;
    use serde_json::json;

    use super::*;
    use crate::http::tests::FakeHttp;

    const VIEWPORT: Viewport = Viewport { size: [10.0, 10.0], pixels: [10, 10] };

    fn app(fake: &FakeHttp) -> App {
        let mut app = App::new();
        app.add_plugins(HttpPlugin::new().backend(Box::new(fake.clone())));
        app
    }

    #[test]
    fn tsx_requests_are_built_and_answered_with_their_own_id() {
        let fake = FakeHttp::default();
        let mut app = app(&fake);
        app.command("http", &json!({ "id": 41, "method": "POST", "url": "http://x/scores", "headers": { "Content-Type": "text/plain" }, "body": "{}", "timeout": 3 }));
        app.command("http", &json!({ "id": 42, "url": "http://x/scores?board=1" }));
        app.tick(VIEWPORT, 0.016);
        let started = fake.started();
        assert_eq!(
            started[0].1,
            HttpRequest { method: Method::Post, url: "http://x/scores".into(), headers: vec![("Content-Type".into(), "text/plain".into())], body: Some("{}".into()), timeout: 3.0 }
        );
        assert_eq!(started[1].1, HttpRequest::get("http://x/scores?board=1"));
        fake.finish(started[1].0, 200, "[]");
        fake.finish(started[0].0, 500, "oops");
        let events = app.tick(VIEWPORT, 0.016);
        assert_eq!(
            events,
            vec![
                ("http".to_owned(), json!({ "id": 42, "ok": true, "status": 200, "body": "[]", "error": null })),
                ("http".to_owned(), json!({ "id": 41, "ok": false, "status": 500, "body": "oops", "error": null })),
            ]
        );
    }

    #[test]
    fn invalid_tsx_requests_get_an_error_reply() {
        let fake = FakeHttp::default();
        let mut app = app(&fake);
        app.command("http", &json!({ "id": 7, "url": "file:///etc/passwd" }));
        let events = app.tick(VIEWPORT, 0.016);
        assert!(fake.started().is_empty());
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].1["id"].clone(), events[0].1["ok"].clone()), (json!(7), json!(false)));
        assert!(events[0].1["error"].as_str().unwrap().contains("http"));
    }

    #[test]
    fn rust_requests_come_back_as_messages_only() {
        let fake = FakeHttp::default();
        let mut app = app(&fake);
        let id = app.world_mut().resource_mut::<Http>().send(HttpRequest::get("http://x/"));
        app.tick(VIEWPORT, 0.016);
        fake.finish(id, 200, "hi");
        let events = app.tick(VIEWPORT, 0.016);
        assert!(events.is_empty(), "不发给 TSX");
        let done: Vec<HttpDone> = app.world_mut().resource_mut::<Messages<HttpDone>>().drain().collect();
        assert_eq!(done, vec![HttpDone { id, status: 200, body: "hi".into(), error: None }]);
    }
}
