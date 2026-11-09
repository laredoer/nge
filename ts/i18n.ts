// 本地化（见 docs/localization.md）：按键取文案、`{参数}`、按语言的复数规则，默认跟着系统语言。
import { locale as systemLocale, signal } from "nsh";

export type PluralCategory = "zero" | "one" | "two" | "few" | "many" | "other";

/** 一条文案：字符串，或按复数类别写的对象（按参数 `count` 选，至少要有 `other`）。 */
export type Message = string | ({ other: string } & Partial<Record<PluralCategory, string>>);

/** 某种语言的文案表：键和默认语言的一样，可以缺。 */
export type Messages<M> = { [K in keyof M]?: Message };

export type Params = Record<string, string | number>;

export interface I18nOptions<M extends Record<string, Message>> {
  /** 语言标签 → 文案表；`fallback` 那份是全的。 */
  messages: Record<string, Messages<M>> & Record<string, unknown>;
  /** 默认语言（`messages` 的键）。 */
  fallback: string;
  /** 系统语言；默认是 nsh 的 `locale`。 */
  system?: () => string;
}

export interface I18n<M> {
  t(key: keyof M & string, params?: Params): string;
  /** 现在用的语言（`messages` 的键）。 */
  locale(): string;
  /** 指定语言；`null` 回到跟着系统。 */
  setLocale(tag: string | null): void;
  locales: string[];
}

const norm = (tag: string) => tag.replace(/_/g, "-").toLowerCase();
const languageOf = (tag: string) => norm(tag).split("-")[0];

/** 在 `available` 里找最合适的语言：完全一样 → 一段段截短 → 同语言别的地区 → `fallback`。 */
export function resolveLocale(tag: string, available: readonly string[], fallback: string): string {
  const find = (want: string) => available.find((a) => norm(a) === want);
  const parts = withScript(norm(tag).split("-").filter((p) => p !== ""));
  for (let n = parts.length; n > 0; n--) {
    const hit = find(parts.slice(0, n).join("-"));
    if (hit !== undefined) return hit;
  }
  if (parts.length > 0) {
    // 同语言别的地区：文字一样的优先（台湾的繁体不落到大陆的简体上）
    const same = available.filter((a) => languageOf(a) === parts[0]);
    const script = parts.find((p) => p.length === 4);
    const sameScript = script === undefined ? undefined : same.find((a) => withScript(norm(a).split("-")).includes(script));
    if (sameScript !== undefined || same.length > 0) return sameScript ?? same[0];
  }
  return fallback;
}

/** 中文没写文字（Hans / Hant）时按地区补上：台湾、香港、澳门是繁体，其他是简体。 */
const HANT_REGIONS = new Set(["tw", "hk", "mo"]);
function withScript(parts: string[]): string[] {
  if (parts[0] !== "zh" || parts.some((p) => p.length === 4)) return parts;
  const region = parts.find((p, i) => i > 0 && p.length === 2);
  if (region === undefined) return parts;
  return [parts[0], HANT_REGIONS.has(region) ? "hant" : "hans", ...parts.slice(1)];
}

const NO_PLURAL = new Set(["zh", "ja", "ko", "th", "vi", "id", "ms"]);

/** 数字 `n` 在语言 `tag` 里的复数类别（CLDR 规则的常用部分）。 */
export function plural(tag: string, n: number): PluralCategory {
  const lang = languageOf(tag);
  const abs = Math.abs(n);
  const int = Number.isInteger(abs);
  const i = Math.floor(abs);
  if (NO_PLURAL.has(lang)) return "other";
  // 法语、西语、意语、葡语：整百万是 many
  const million = int && i !== 0 && i % 1000000 === 0;
  // 巴西葡萄牙语和法语：0 到 2 之间（不含 2）都是单数；别的地区的葡语按葡萄牙的规则
  const brazil = lang === "pt" && (norm(tag) === "pt" || norm(tag) === "pt-br");
  if (lang === "fr" || brazil) return i === 0 || i === 1 ? "one" : million ? "many" : "other";
  if (["es", "it", "pt"].includes(lang)) return abs === 1 ? "one" : million ? "many" : "other";
  // 丹麦语：1，以及整数部分是 0 或 1 的小数
  if (lang === "da") return abs === 1 || (!int && (i === 0 || i === 1)) ? "one" : "other";
  if (lang === "ru" || lang === "uk" || lang === "pl") {
    if (!int) return "other";
    const [d, h] = [i % 10, i % 100];
    if (lang === "pl" ? i === 1 : d === 1 && h !== 11) return "one";
    if (d >= 2 && d <= 4 && !(h >= 12 && h <= 14)) return "few";
    return "many";
  }
  if (lang === "ar") {
    if (!int) return "other";
    const h = i % 100;
    if (i === 0) return "zero";
    if (i === 1) return "one";
    if (i === 2) return "two";
    if (h >= 3 && h <= 10) return "few";
    if (h >= 11) return "many";
    return "other";
  }
  return abs === 1 ? "one" : "other";
}

/** 把 `{名字}` 换成参数，`{{` / `}}` 是字面的花括号。 */
const formats = new Map<string, (n: number) => string>();
function formatter(tag: string): (n: number) => string {
  let f = formats.get(tag);
  if (f === undefined) {
    try {
      const nf = new Intl.NumberFormat(tag);
      f = (n) => nf.format(n);
    } catch {
      f = String;
    }
    formats.set(tag, f);
  }
  return f;
}

/** 只读游戏给的参数，不读到原型链上的 `constructor` 这些。 */
const param = (params: Params, name: string) => (Object.prototype.hasOwnProperty.call(params, name) ? params[name] : undefined);

function fill(text: string, params: Params, tag: string): string {
  const number = formatter(tag);
  return text.replace(/\{\{|\}\}|\{([^{}]+)\}/g, (all, name: string | undefined) => {
    if (name === undefined) return all[0];
    const v = param(params, name);
    if (v === undefined) return all;
    return typeof v === "number" ? number(v) : v;
  });
}

const pick = (m: Message, params: Params, tag: string): string => {
  if (typeof m === "string") return m;
  const count = param(params, "count");
  return typeof count === "number" ? (m[plural(tag, count)] ?? m.other) : m.other;
};

export function i18n<M extends Record<string, Message>>(options: I18nOptions<M>): I18n<M> {
  const { messages, fallback } = options;
  const locales = Object.keys(messages);
  if (!locales.includes(fallback)) throw new RangeError(`默认语言 ${fallback} 不在文案表里（有 ${locales.join("、")}）`);
  const system = options.system ?? systemLocale;
  const chosen = signal<string | null>(null);
  const warned = new Set<string>();
  const locale = () => resolveLocale(chosen() ?? system(), locales, fallback);
  return {
    locales,
    locale,
    // 存档读回来的不一定是字符串：不是字符串的当成跟系统
    setLocale: (tag) => chosen.set(typeof tag === "string" ? resolveLocale(tag, locales, fallback) : null),
    t(key, params = {}) {
      const tag = locale();
      const own = (messages[tag] as Messages<M>)[key];
      // 退到默认语言的文案按默认语言的复数规则选
      const from = own !== undefined ? tag : fallback;
      const m = own ?? (messages[fallback] as Messages<M>)[key];
      if (m === undefined) {
        if (!warned.has(key)) {
          warned.add(key);
          console.warn(`没有文案 ${key}`);
        }
        return key;
      }
      return fill(pick(m, params, from), params, tag);
    },
  };
}

/** 每种语言比默认语言缺哪些键（复数文案没有 `other` 也算缺）；不缺的语言（包括默认语言自己）不列。 */
export function missing<M extends Record<string, Message>>(messages: Record<string, Messages<M>>, fallback: string): Record<string, string[]> {
  const keys = Object.keys(messages[fallback] ?? {});
  const out: Record<string, string[]> = {};
  for (const [tag, table] of Object.entries(messages)) {
    const lack = keys.filter((k) => {
      const m = (table as Record<string, Message | undefined>)[k];
      return m === undefined || (typeof m === "object" && typeof m.other !== "string");
    });
    if (lack.length > 0) out[tag] = lack;
  }
  return out;
}
