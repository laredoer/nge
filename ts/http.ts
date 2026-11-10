// HTTP（见 docs/networking.md）：经引擎发 GET / POST，原生和浏览器一样；结果是 Promise。
import type { NativeComponentRef } from "nsh";

export interface HttpOptions {
  headers?: Record<string, string>;
  /** 秒，默认 10，最多 60。 */
  timeout?: number;
}

/** 一个请求的结果：`ok` 是拿到了 2xx；`error` 是没拿到响应（地址不合规、连不上、超时、响应超过 1 MB）。 */
export interface HttpResult {
  ok: boolean;
  status: number;
  body: string;
  error?: string;
}

/** 引擎发来的 `http` 事件。 */
export interface HttpReply {
  id: number;
  ok: boolean;
  status: number;
  body: string;
  error: string | null;
}

export interface HttpClient {
  get(url: string, options?: HttpOptions): Promise<HttpResult>;
  /** `body` 不是字符串时编成 JSON。默认 `Content-Type: text/plain`（浏览器跨域时不用预检）。 */
  post(url: string, body: unknown, options?: HttpOptions): Promise<HttpResult>;
  /** 接 `<GameView onHttp={http.handle}>`。 */
  handle(reply: HttpReply): void;
  /** 还没回来的请求数。 */
  pending(): number;
}

/** 引擎的超时之外再等多久（毫秒）：引擎一直不回（command 丢了、视图没了）时按出错结束。 */
const GRACE_MS = 2000;
const DEFAULT_TIMEOUT = 10;

/** `game` 拿游戏视图（`<GameView ref>`）；还没建好时请求直接以 `error` 结束。 */
export function httpClient(game: () => NativeComponentRef | undefined, settings: { graceMs?: number } = {}): HttpClient {
  let next = 1;
  const waiting = new Map<number, { resolve: (r: HttpResult) => void; timer: ReturnType<typeof setTimeout> }>();
  const request = (method: "GET" | "POST", url: string, body: string | undefined, options: HttpOptions = {}) =>
    new Promise<HttpResult>((resolve) => {
      const view = game();
      if (view === undefined) return resolve({ ok: false, status: 0, body: "", error: "游戏视图还没建好" });
      const id = next++;
      const wait = (options.timeout ?? DEFAULT_TIMEOUT) * 1000 + (settings.graceMs ?? GRACE_MS);
      const timer = setTimeout(() => {
        if (!waiting.delete(id)) return;
        resolve({ ok: false, status: 0, body: "", error: "引擎没有回应" });
      }, wait);
      waiting.set(id, { resolve, timer });
      view.command("http", { id, method, url, headers: options.headers ?? {}, body, timeout: options.timeout });
    });
  return {
    get: (url, options) => request("GET", url, undefined, options),
    post: (url, body, options = {}) =>
      request("POST", url, typeof body === "string" ? body : JSON.stringify(body), { ...options, headers: withDefault(options.headers ?? {}, "Content-Type", "text/plain") }),
    handle(reply) {
      const entry = waiting.get(reply.id);
      if (entry === undefined) return;
      waiting.delete(reply.id);
      clearTimeout(entry.timer);
      entry.resolve(reply.error === null ? { ok: reply.ok, status: reply.status, body: reply.body } : { ok: false, status: reply.status, body: reply.body, error: reply.error });
    },
    pending: () => waiting.size,
  };
}

/** 没有同名（不分大小写）的头时加上默认的。 */
function withDefault(headers: Record<string, string>, name: string, value: string): Record<string, string> {
  const has = Object.keys(headers).some((k) => k.toLowerCase() === name.toLowerCase());
  return has ? headers : { [name]: value, ...headers };
}

/** 把响应体当 JSON 读；不是 2xx 或者解不出来时是 `undefined`。 */
export function json<T>(result: HttpResult): T | undefined {
  if (!result.ok) return undefined;
  try {
    return JSON.parse(result.body) as T;
  } catch {
    return undefined;
  }
}
