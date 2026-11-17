// 平台跳跃的文案：英文一条不缺，两种语言的参数一样。
import { expect, test } from "nsh/test";
import { missing, type Message } from "../../ts/nge";
import { LEVELS } from "./levels";
import { EN, levelKey, MESSAGES, otherLocale, strings, ZH } from "./strings";

/** 一条文案里用到的参数名（复数文案的每个类别合起来）。 */
function params(m: Message | undefined): string[] {
  const texts = m === undefined ? [] : typeof m === "string" ? [m] : Object.values(m).filter((v): v is string => typeof v === "string");
  return [...new Set(texts.flatMap((s) => [...s.matchAll(/\{(\w+)\}/g)].map((x) => x[1])))].sort();
}

test("英文一条不缺，每条的参数和中文一样", () => {
  expect(missing(MESSAGES, "zh")).toEqual({});
  for (const key of Object.keys(ZH) as (keyof typeof ZH)[]) {
    expect(`${key}: ${params(EN[key]).join(",")}`).toBe(`${key}: ${params(ZH[key]).join(",")}`);
  }
});

test("每关都有名字；跟系统语言，英文按复数", () => {
  const en = strings(() => "en-GB");
  for (let i = 0; i < LEVELS.length; i++) expect(en.t(levelKey(i))).not.toBe(levelKey(i));
  expect(en.t("falls", { count: 1 })).toBe("1 fall");
  expect(en.t("falls", { count: 3 })).toBe("3 falls");
  expect(strings(() => "zh-Hans-CN").t("falls", { count: 1 })).toBe("失手 1 次");
  expect(strings(() => "fr").locale()).toBe("zh");
});

test("中文关卡名和关卡表里的名字一样", () => {
  const zh = strings(() => "zh");
  expect(LEVELS.map((_, i) => zh.t(levelKey(i)))).toEqual(LEVELS.map((l) => l.name));
});

test("L 键在中文和英文之间切", () => {
  expect(otherLocale("zh")).toBe("en");
  expect(otherLocale("en")).toBe("zh");
});
