// 长坂坡 web 版的加载页（nsh.json 的 web.host，nsh package 原样带上，见 docs/web.md）。
// 页面一开始就盖在最上面：先显示下载引擎（wasm）的进度（nsh.js 发的 nsh-load-progress），
// 再显示取游戏资源的进度（引擎发的 nge-progress），游戏画出第一帧（nge-ready）后淡出。
// 起不来时（nge-error，或者页面自己的错误框）显示原因和「重试」。

/** 引擎占进度条的多少，剩下的是资源。 */
const ENGINE_SHARE = 0.85;
/** 压缩传输时 Content-Length 是压缩后的大小，按常见的压缩比估原始大小。 */
const RATIO = { br: 3.7, gzip: 2.7, deflate: 2.7 };
/** 拿不到大小时按这么大估（字节），进度条慢慢逼近但不到头。 */
const GUESS = 12e6;

const MB = (bytes) => (bytes / 1e6).toFixed(1);

/** 引擎下载了多少（0–1）和要显示的字。 */
export function engineProgress({ loaded = 0, length = 0, encoding = "", done = false } = {}) {
  if (done) return { part: 1, text: `下载引擎 ${MB(loaded)} MB` };
  const ratio = encoding ? RATIO[encoding.toLowerCase()] : 1;
  const total = length && ratio ? length * ratio : 0;
  if (total && loaded <= total) {
    return { part: Math.min(loaded / total, 0.99), text: `下载引擎 ${MB(loaded)} / ${MB(total)} MB` };
  }
  // 大小不知道或者对不上：按收到的字节慢慢逼近
  return { part: Math.min(1 - Math.exp(-loaded / Math.max(total, GUESS)), 0.95), text: `下载引擎 ${MB(loaded)} MB` };
}

const style = `
#nge-loading { position: fixed; inset: 0; z-index: 2147483000; display: flex; flex-direction: column;
  align-items: center; justify-content: center; gap: 18px; padding: 24px; box-sizing: border-box;
  background: radial-gradient(ellipse at 50% 40%, #2a1d16 0%, #120c09 70%); color: #f3e3c3;
  font: 14px/1.5 system-ui, -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif; transition: opacity .45s ease; }
#nge-loading.gone { opacity: 0; pointer-events: none; }
#nge-loading h1 { margin: 0; font-size: clamp(32px, 9vw, 56px); letter-spacing: .3em; padding-left: .3em;
  font-weight: 700; color: #e9c46a; text-shadow: 0 2px 18px rgba(233, 140, 40, .35); }
#nge-loading .bar { width: min(340px, 78vw); height: 8px; border-radius: 4px; background: rgba(243, 227, 195, .12); overflow: hidden; }
#nge-loading .fill { height: 100%; width: 0; border-radius: 4px; background: linear-gradient(90deg, #b8742a, #f0c25a); transition: width .2s linear; }
#nge-loading .text { min-height: 1.5em; font-size: 13px; opacity: .8; font-variant-numeric: tabular-nums; text-align: center; }
#nge-loading .error { max-width: min(520px, 86vw); color: #ffb4a2; text-align: center; white-space: pre-wrap; word-break: break-word; }
#nge-loading button { font: inherit; color: #120c09; background: #e9c46a; border: 0; border-radius: 6px; padding: 8px 26px; cursor: pointer; }
#nge-loading button:hover { background: #f3d58a; }
`;

function mountOverlay() {
  const css = document.createElement("style");
  css.textContent = style;
  document.head.appendChild(css);
  const root = document.createElement("div");
  root.id = "nge-loading";
  root.setAttribute("role", "status");
  root.innerHTML = `<h1>长坂坡</h1><div class="bar"><div class="fill"></div></div><div class="text">准备下载…</div>`;
  document.body.appendChild(root);
  const fill = root.querySelector(".fill");
  const text = root.querySelector(".text");
  let engine = { part: 0 };
  let failed = false;

  const show = (part, label) => {
    if (failed) return;
    fill.style.width = `${Math.round(Math.max(0, Math.min(part, 1)) * 100)}%`;
    text.textContent = label;
  };
  const onEngine = (detail) => {
    engine = engineProgress(detail);
    show(engine.part * ENGINE_SHARE, detail?.done ? "启动引擎…" : engine.text);
  };
  const fail = (message) => {
    if (failed) return;
    failed = true;
    root.setAttribute("role", "alert");
    const error = document.createElement("div");
    error.className = "error";
    error.textContent = message || "加载失败";
    const retry = document.createElement("button");
    retry.textContent = "重试";
    retry.addEventListener("click", () => location.reload());
    root.querySelector(".bar").replaceWith(error);
    text.replaceWith(retry);
  };

  if (globalThis.nshLoadProgress) onEngine(globalThis.nshLoadProgress);
  addEventListener("nsh-load-progress", (e) => onEngine(e.detail));
  addEventListener("nge-progress", (e) => {
    const { done = 0, total = 0 } = e.detail ?? {};
    const part = total ? done / total : 1;
    show(ENGINE_SHARE + (1 - ENGINE_SHARE) * part, total ? `加载资源 ${done} / ${total}` : "加载资源…");
  });
  addEventListener("nge-ready", () => {
    show(1, "");
    root.classList.add("gone");
    setTimeout(() => root.remove(), 600);
  });
  addEventListener("nge-error", (e) => fail(e.detail?.message));
  // 页面自己的错误框（下载失败、运行时起不来）
  const status = document.getElementById("status");
  if (status) {
    const check = () => status.classList.contains("failed") && fail(status.textContent.trim());
    new MutationObserver(check).observe(status, { attributes: true, childList: true, subtree: true });
    check();
  }
}

if (typeof document !== "undefined") mountOverlay();

/** nsh 在应用启动前调用；长坂坡不用 `nsh/host`。 */
export function createHost() {
  return { call: () => null };
}
