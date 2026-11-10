//! 浏览器：`fetch`，超时用 `AbortController`；结果放进队列，引擎每帧取。

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, Headers, Request, RequestInit, Response};

use super::{HttpBackend, HttpDone, HttpRequest, MAX_RESPONSE, Method};

#[derive(Default)]
pub struct WebHttp {
    done: Rc<RefCell<Vec<HttpDone>>>,
}

// 浏览器上只有一个线程：Send + Sync 只是为了放进 ECS 的资源（同 docs/web.md 里 wgpu 的做法）
unsafe impl Send for WebHttp {}
unsafe impl Sync for WebHttp {}

impl HttpBackend for WebHttp {
    fn start(&mut self, id: u64, request: HttpRequest) {
        let done = self.done.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch(&request).await.unwrap_or_else(|e| Err(format!("请求 {} 失败: {}", request.url, describe(&e))));
            done.borrow_mut().push(match result {
                Ok((status, body)) => HttpDone { id, status, body, error: None },
                Err(e) => HttpDone::failed(id, e),
            });
        });
    }

    fn poll(&mut self) -> Vec<HttpDone> {
        std::mem::take(&mut *self.done.borrow_mut())
    }
}

fn describe(e: &JsValue) -> String {
    e.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())).or_else(|| e.as_string()).unwrap_or_else(|| format!("{e:?}"))
}

/// 外层的错是 JS 抛的（连不上、被中止），里层的是我们自己判的（响应太大）。
async fn fetch(request: &HttpRequest) -> Result<Result<(u16, String), String>, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("没有 window"))?;
    let init = RequestInit::new();
    init.set_method(match request.method {
        Method::Get => "GET",
        Method::Post => "POST",
    });
    let headers = Headers::new()?;
    for (k, v) in &request.headers {
        headers.append(k, v)?;
    }
    init.set_headers(&headers);
    if let Some(body) = &request.body {
        init.set_body(&JsValue::from_str(body));
    }
    let abort = AbortController::new()?;
    init.set_signal(Some(&abort.signal()));
    let timer = {
        let abort = abort.clone();
        Closure::once_into_js(move || abort.abort())
    };
    let timeout = window.set_timeout_with_callback_and_timeout_and_arguments_0(timer.unchecked_ref(), (request.timeout * 1000.0) as i32)?;
    let req = Request::new_with_str_and_init(&request.url, &init)?;
    let result = async {
        let response: Response = JsFuture::from(window.fetch_with_request(&req)).await?.dyn_into()?;
        let status = response.status();
        let text = JsFuture::from(response.text()?).await?.as_string().unwrap_or_default();
        Ok::<_, JsValue>(if text.len() as u64 > MAX_RESPONSE { Err(format!("响应超过 {MAX_RESPONSE} 字节")) } else { Ok((status, text)) })
    }
    .await;
    window.clear_timeout_with_handle(timeout);
    result
}
