// 预制体实例的覆盖（docs/editor-more.md）：`set: { 子节点路径: { 字段: 值 } }`，在属性面板里一条条加、删。
import { CHILD_PATH, type Entity, type PropValue } from "../../ts/level";
import type { Parsed } from "./fields";

const FIELD = /^[A-Za-z][A-Za-z0-9_]{0,31}$/;

/** 一条覆盖的输入：路径、字段名按文件格式的规则；值按「数 → 布尔 → 文字」认。 */
export function parseOverride(path: string, field: string, text: string): Parsed<{ path: string; field: string; value: PropValue }> {
  const p = path.trim();
  const f = field.trim();
  if (!CHILD_PATH.test(p)) return { ok: false, error: "子节点要是 id 用 / 连起来，比如 b/crown（不能有 __proto__ 这一段）" };
  if (!FIELD.test(f)) return { ok: false, error: "字段名要是字母开头的字母、数字、下划线" };
  const t = text.trim();
  if (t.length > 256 || /[\u0000-\u001f\u007f]/.test(t)) return { ok: false, error: "值太长或有控制字符" };
  const n = t === "" ? NaN : Number(t);
  const value: PropValue = Number.isFinite(n) ? n : t === "true" ? true : t === "false" ? false : t;
  if (typeof value === "number" && Math.abs(value) > 1e6) return { ok: false, error: "数的绝对值不超过 1000000" };
  return { ok: true, value: { path: p, field: f, value } };
}

/** 加上（或换掉）一条覆盖：返回新的 `set`。 */
export function withOverride(set: Entity["set"], path: string, field: string, value: PropValue): NonNullable<Entity["set"]> {
  return { ...set, [path]: { ...set?.[path], [field]: value } };
}

/** 去掉一条覆盖；子节点下面没有了就去掉子节点，全空了是 undefined。 */
export function withoutOverride(set: Entity["set"], path: string, field: string): Entity["set"] {
  if (set?.[path]?.[field] === undefined) return set;
  const { [field]: _gone, ...fields } = set[path];
  const { [path]: _child, ...rest } = set;
  const next = Object.keys(fields).length === 0 ? rest : { ...rest, [path]: fields };
  return Object.keys(next).length === 0 ? undefined : next;
}

/** 覆盖一条条列出来（属性面板用）。 */
export function overrideList(set: Entity["set"]): { path: string; field: string; value: PropValue }[] {
  return Object.entries(set ?? {}).flatMap(([path, fields]) => Object.entries(fields).map(([field, value]) => ({ path, field, value })));
}
