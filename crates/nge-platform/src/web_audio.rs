//! 页面上的音频设备（`AudioContext`）：kira 经 cpal 建的，不经过我们的代码。在原型的
//! `resume` 上记下用过的设备，好在页面隐藏时挂起、回来时恢复，用户再次点按时把没在跑的拉起来。
//! cpal 开始播放时会调 `resume`，所以记得到；`close` 掉的不再记。

use wasm_bindgen::JsValue;

/// `op`：`track` 只装钩子；`suspend` 挂起在跑的；`resume` 恢复没在跑（也没关掉）的。
const BODY: &str = r#"
const P = globalThis.AudioContext && globalThis.AudioContext.prototype;
if (!P) return;
if (!P.__ngeSeen) {
  const seen = new Set();
  const resume = P.resume;
  const close = P.close;
  P.resume = function () { seen.add(this); return resume.apply(this, arguments); };
  P.close = function () { seen.delete(this); return close.apply(this, arguments); };
  Object.defineProperty(P, "__ngeSeen", { value: seen });
}
const quiet = () => {};
for (const c of P.__ngeSeen) {
  if (op === "suspend" && c.state === "running") c.suspend().catch(quiet);
  if (op === "resume" && c.state !== "running" && c.state !== "closed") c.resume().catch(quiet);
}
"#;

fn call(op: &str) {
    let f = js_sys::Function::new_with_args("op", BODY);
    if let Err(e) = f.call1(&JsValue::NULL, &op.into()) {
        log::warn!("音频设备的钩子出错：{e:?}");
    }
}

/// 在 kira 建设备之前装上钩子。
pub(crate) fn track() {
    call("track");
}

/// 页面隐藏：挂起。
pub(crate) fn suspend() {
    call("suspend");
}

/// 页面回来、用户点按：恢复。
pub(crate) fn resume() {
    call("resume");
}
