// 本地化（见 docs/localization.md）：文案、复数、语言选择。
import { signal } from "nsh";
import { expect, test } from "nsh/test";
import { i18n, missing, plural, resolveLocale, type Messages } from "../../ts/i18n";

const ZH = {
  hello: "你好，{name}",
  coins: "金币 {got} / {all}",
  fell: { other: "失手 {count} 次" },
  brace: "{{字面}} {x}",
  onlyZh: "只有中文",
};
const EN: Messages<typeof ZH> = {
  hello: "Hello, {name}",
  coins: "Coins {got} / {all}",
  fell: { one: "{count} fall", other: "{count} falls" },
};

function make(system = "zh-Hans-CN") {
  const sys = signal(system);
  const lang = i18n({ messages: { zh: ZH, en: EN, ru: { fell: { one: "{count} падение", few: "{count} падения", many: "{count} падений", other: "{count} падения" } } }, fallback: "zh", system: () => sys() });
  return { lang, sys };
}

test("参数替换：数字按语言格式化，缺的参数原样留着，双花括号是字面的", () => {
  const { lang } = make();
  expect(lang.t("hello", { name: "小明" })).toBe("你好，小明");
  expect(lang.t("hello")).toBe("你好，{name}");
  expect(lang.t("brace", { x: 1 })).toBe("{字面} 1");
  lang.setLocale("en");
  expect(lang.t("coins", { got: 1234, all: 2000 })).toBe("Coins 1,234 / 2,000");
});

test("复数按语言选，没写的类别用 other，没给 count 用 other", () => {
  const { lang } = make("en-US");
  expect(lang.t("fell", { count: 1 })).toBe("1 fall");
  expect(lang.t("fell", { count: 0 })).toBe("0 falls");
  expect(lang.t("fell", { count: 2 })).toBe("2 falls");
  expect(lang.t("fell")).toBe("{count} falls");
  lang.setLocale("ru");
  expect([1, 3, 5, 11, 21, 22, 1.5].map((count) => lang.t("fell", { count }).split(" ")[1])).toEqual([
    "падение", "падения", "падений", "падений", "падение", "падения", "падения",
  ]);
  lang.setLocale("zh");
  expect(lang.t("fell", { count: 1 })).toBe("失手 1 次");
});

test("复数文案没写这个类别时用 other；默认语言不在表里直接抛", () => {
  const lang = i18n({ messages: { en: { x: { other: "{count} things" } } }, fallback: "en", system: () => "en" });
  expect(lang.t("x", { count: 1 })).toBe("1 things");
  let threw = false;
  try {
    i18n({ messages: { en: {} }, fallback: "zh", system: () => "en" });
  } catch {
    threw = true;
  }
  expect(threw).toBe(true);
});

test("复数规则表", () => {
  expect([0, 1, 2].map((n) => plural("en", n))).toEqual(["other", "one", "other"]);
  expect([0, 1, 1.5, 2].map((n) => plural("fr", n))).toEqual(["one", "one", "one", "other"]);
  expect([1, 2, 5, 12, 21, 22, 25, 111].map((n) => plural("pl", n))).toEqual(["one", "few", "many", "many", "many", "few", "many", "many"]);
  expect([1, 21, 11, 2, 14, 0].map((n) => plural("uk", n))).toEqual(["one", "one", "many", "few", "many", "many"]);
  expect([0, 1, 2, 5, 10, 11, 99, 100, 102].map((n) => plural("ar", n))).toEqual(["zero", "one", "two", "few", "few", "many", "many", "other", "other"]);
  expect([1, 2].map((n) => plural("ja", n))).toEqual(["other", "other"]);
  expect(plural("xx", 1)).toBe("one");
  expect(plural("pt-PT", 0)).toBe("other");
  expect(plural("pt-BR", 0)).toBe("one");
  expect(plural("en", -1)).toBe("one");
});

test("语言选择：完全一样、一段段截短、同语言别的地区、大小写和下划线、找不到用默认", () => {
  const all = ["zh", "en-US", "zh-Hant", "pt-BR"];
  expect(resolveLocale("en-US", all, "zh")).toBe("en-US");
  expect(resolveLocale("zh-Hans-CN", all, "zh")).toBe("zh");
  expect(resolveLocale("zh-Hant-TW", all, "zh")).toBe("zh-Hant");
  expect(resolveLocale("en-GB", all, "zh")).toBe("en-US");
  expect(resolveLocale("EN_us", all, "zh")).toBe("en-US");
  expect(resolveLocale("fr-FR", all, "zh")).toBe("zh");
  expect(resolveLocale("", all, "zh")).toBe("zh");
  expect(resolveLocale("pt", all, "zh")).toBe("pt-BR");
});

test("跟着系统语言；setLocale 指定，null 回到系统；缺的文案退到默认语言", () => {
  const { lang, sys } = make("en-AU");
  expect(lang.locale()).toBe("en");
  expect(lang.locales).toEqual(["zh", "en", "ru"]);
  expect(lang.t("onlyZh")).toBe("只有中文");
  sys.set("zh-CN");
  expect(lang.locale()).toBe("zh");
  lang.setLocale("en");
  sys.set("ru");
  expect(lang.locale()).toBe("en");
  lang.setLocale(null);
  expect(lang.locale()).toBe("ru");
  expect(lang.t("hello", { name: "x" })).toBe("你好，x");
});

test("都没有的键返回键名，只警告一次", () => {
  const { lang } = make();
  const warned: unknown[] = [];
  const warn = console.warn;
  console.warn = (...a: unknown[]) => void warned.push(a);
  try {
    const t = lang.t as (key: string) => string;
    expect(t("nope")).toBe("nope");
    expect(t("nope")).toBe("nope");
    expect(warned.length).toBe(1);
  } finally {
    console.warn = warn;
  }
});

test("missing 找出每种语言缺的键和没有 other 的复数文案", () => {
  expect(missing({ zh: ZH, en: EN }, "zh")).toEqual({ en: ["brace", "onlyZh"] });
  const broken = { zh: ZH, en: { ...EN, brace: "b", onlyZh: "o", fell: { one: "x" } } } as unknown as Record<string, Messages<typeof ZH>>;
  expect(missing(broken, "zh")).toEqual({ en: ["fell"] });
  expect(missing({ zh: ZH }, "zh")).toEqual({});
});

test("退到默认语言的文案按默认语言的复数规则选；只读自己的参数；不是字符串的语言当跟系统", () => {
  const lang = i18n({ messages: { en: { fall: { one: "{count} fall", other: "{count} falls" }, odd: "{constructor}-{toString}" }, ru: {} }, fallback: "en", system: () => "ru" });
  expect(lang.t("fall", { count: 21 })).toBe("21 falls");
  expect(lang.t("odd")).toBe("{constructor}-{toString}");
  lang.setLocale(1 as unknown as string);
  expect(lang.locale()).toBe("ru");
});

test("中文没写文字时按地区补：台港澳繁体，其他简体", () => {
  expect(resolveLocale("zh-TW", ["zh-Hans", "zh-Hant"], "zh-Hans")).toBe("zh-Hant");
  expect(resolveLocale("zh-HK", ["zh-Hans", "zh-Hant"], "zh-Hans")).toBe("zh-Hant");
  expect(resolveLocale("zh-CN", ["zh-Hant", "zh-Hans"], "zh-Hant")).toBe("zh-Hans");
  expect(resolveLocale("zh-Hant-HK", ["zh-CN", "zh-TW"], "zh-CN")).toBe("zh-TW");
  expect(resolveLocale("zh", ["en", "zh-Hant"], "en")).toBe("zh-Hant");
  expect(resolveLocale("zh-Hans-TW", ["zh-Hant", "zh-Hans"], "zh-Hant")).toBe("zh-Hans");
});

test("法西意葡的整百万是 many；丹麦语的小数；葡语除巴西外按葡萄牙", () => {
  expect([1, 2, 1000000, 0].map((n) => plural("fr", n))).toEqual(["one", "other", "many", "one"]);
  expect([1, 1000000, 2000000, 1000001].map((n) => plural("es", n))).toEqual(["one", "many", "many", "other"]);
  expect(plural("it", 1000000)).toBe("many");
  expect([0.5, 1.5, 1, 2, 0].map((n) => plural("da", n))).toEqual(["one", "one", "one", "other", "other"]);
  expect([plural("pt-AO", 1.5), plural("pt", 1.5), plural("pt-BR", 0), plural("pt-PT", 1000000)]).toEqual(["other", "one", "one", "many"]);
});
