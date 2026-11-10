//! 原生：每个请求一个线程跑 ureq（阻塞调用在那个线程里，不在引擎线程上），结果从通道回来。

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use super::{HttpBackend, HttpDone, HttpRequest, MAX_RESPONSE, Method, too_large};

pub struct NativeHttp {
    tx: Sender<HttpDone>,
    rx: Mutex<Receiver<HttpDone>>,
}

impl Default for NativeHttp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx: Mutex::new(rx) }
    }
}

impl HttpBackend for NativeHttp {
    fn start(&mut self, id: u64, request: HttpRequest) {
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new().name("nge-http".into()).spawn(move || {
            let _ = tx.send(fetch(id, &request));
        });
        if let Err(e) = spawned {
            let _ = self.tx.send(HttpDone::failed(id, format!("起不了请求线程: {e}")));
        }
    }

    fn poll(&mut self) -> Vec<HttpDone> {
        self.rx.lock().map(|rx| rx.try_iter().collect()).unwrap_or_default()
    }
}

fn fetch(id: u64, request: &HttpRequest) -> HttpDone {
    let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs_f32(request.timeout))).http_status_as_error(false).build();
    let agent = ureq::Agent::new_with_config(config);
    let result = match request.method {
        Method::Get => request.headers.iter().fold(agent.get(&request.url), |r, (k, v)| r.header(k, v)).call(),
        Method::Post => {
            let builder = request.headers.iter().fold(agent.post(&request.url), |r, (k, v)| r.header(k, v));
            builder.send(request.body.as_deref().unwrap_or_default())
        }
    };
    let mut response = match result {
        Ok(r) => r,
        Err(e) => return HttpDone::failed(id, format!("请求 {} 失败: {e}", request.url)),
    };
    let status = response.status().as_u16();
    // 按字节读，不是 UTF-8 的按替换字符解（和浏览器的 text() 一样）
    match response.body_mut().with_config().limit(MAX_RESPONSE).read_to_vec() {
        Ok(bytes) => HttpDone { id, status, body: String::from_utf8_lossy(&bytes).into_owned(), error: None },
        Err(ureq::Error::BodyExceedsLimit(_)) => HttpDone { id, status, body: String::new(), error: Some(too_large()) },
        Err(e) => HttpDone { id, status, body: String::new(), error: Some(format!("读响应失败: {e}")) },
    }
}
