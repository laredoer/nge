// 属性面板的输入（docs/editor.md）：文字 → 实体的改动，不合规的给出原因、不改。
import { checkProp, MAX_COLS, MAX_COORD, MAX_PARALLAX, MAX_ROTATION, MAX_ROWS, type Entity, type PropValue } from "../../ts/level";
import { propSpec, TYPES } from "../platformer/catalog";

export type Parsed<T> = { ok: true; value: T } | { ok: false; error: string };

/** 数：空的、不是数、超出范围的不要。 */
export function parseNumber(text: string, min: number, max: number): Parsed<number> {
  const t = text.trim();
  const v = t === "" ? NaN : Number(t);
  if (!Number.isFinite(v)) return { ok: false, error: "要是数" };
  if (v < min || v > max) return { ok: false, error: `要在 ${min} 到 ${max} 之间` };
  return { ok: true, value: v };
}

/** 整数。 */
export function parseInteger(text: string, min: number, max: number): Parsed<number> {
  const r = parseNumber(text, min, max);
  return r.ok && !Number.isInteger(r.value) ? { ok: false, error: "要是整数" } : r;
}

/** 名字（实体的、关卡的）：不超过 256 个字，没有控制字符；合规返回 null。 */
export function nameError(text: string): string | null {
  return text.length > 256 || /[\u0000-\u001f\u007f]/.test(text) ? "名字太长或有控制字符" : null;
}

/** 属性面板上一个输入框的改动：`field` 是 name、x、y、w、h、rotation、z，或 `prop:<属性名>`。 */
export function fieldPatch(e: Entity, field: string, text: string): Parsed<Partial<Entity>> {
  const spec = TYPES[e.type];
  switch (field) {
    case "name": {
      const why = nameError(text);
      if (why !== null) return { ok: false, error: why };
      return { ok: true, value: { name: text.trim() === "" ? undefined : text } };
    }
    case "x":
    case "y": {
      if (field === "y" && spec?.fixedY !== undefined) return { ok: false, error: "站在地上的东西不能改高度" };
      const r = parseNumber(text, -MAX_COORD, MAX_COORD);
      return r.ok ? { ok: true, value: { [field]: r.value } } : r;
    }
    case "rotation": {
      if (spec?.rotatable !== true) return { ok: false, error: "这种东西不能转" };
      const r = parseNumber(text, -MAX_ROTATION, MAX_ROTATION);
      return r.ok ? { ok: true, value: { rotation: r.value === 0 ? undefined : r.value } } : r;
    }
    case "z": {
      if (spec?.layered !== true) return { ok: false, error: "这种东西的 z 由类型定" };
      // 空的是按类型的
      if (text.trim() === "") return { ok: true, value: { z: undefined } };
      const r = parseNumber(text, -MAX_COORD, MAX_COORD);
      return r.ok ? { ok: true, value: { z: r.value } } : r;
    }
    case "w":
    case "h": {
      if (spec?.resizable === undefined || (field === "h" && spec.resizable !== "wh")) return { ok: false, error: field === "h" && spec?.resizable === "w" ? "这种东西的高是固定的" : "这种东西的大小是固定的" };
      const r = parseNumber(text, 1, MAX_COORD);
      return r.ok ? { ok: true, value: { [field]: r.value } } : r;
    }
  }
  const key = field.startsWith("prop:") ? field.slice(5) : "";
  const prop = propSpec(e.type, key);
  if (prop === undefined) return { ok: false, error: `没有属性 ${key}` };
  let value: PropValue;
  if (prop.kind === "number") {
    const r = parseNumber(text, prop.min, prop.max);
    if (!r.ok) return r;
    value = r.value;
  } else if (prop.kind === "bool") {
    if (text !== "true" && text !== "false") return { ok: false, error: "要是布尔" };
    value = text === "true";
  } else {
    value = text;
  }
  const why = checkProp(prop, value);
  return why !== null ? { ok: false, error: why } : { ok: true, value: { props: { ...e.props, [key]: value } } };
}

/** 编辑器里瓦片层最多多大（比文件格式的上限小：每次拖动都要重建场景）。 */
export const EDIT_MAX_COLS = Math.min(1000, MAX_COLS);
export const EDIT_MAX_ROWS = Math.min(200, MAX_ROWS);

/** 关卡设置里瓦片层的列数、行数。 */
export function parseSize(text: string, rows: boolean): Parsed<number> {
  return parseInteger(text, 1, rows ? EDIT_MAX_ROWS : EDIT_MAX_COLS);
}

/** 视差的一个分量（0 … 4）。 */
export function parseParallax(text: string): Parsed<number> {
  return parseNumber(text, 0, MAX_PARALLAX);
}

/** 可以空着的数（空的是去掉这一项）。 */
export function parseOptional(text: string, min: number, max: number): Parsed<number | undefined> {
  return text.trim() === "" ? { ok: true, value: undefined } : parseNumber(text, min, max);
}

/** 动画帧：逗号或空格隔开的帧号（图集里有的），空的是不动。 */
export function parseFrames(text: string, count: number): Parsed<number[] | undefined> {
  const parts = text.split(/[,，\s]+/).filter((p) => p !== "");
  if (parts.length === 0) return { ok: true, value: undefined };
  if (parts.length > 64) return { ok: false, error: "最多 64 帧" };
  const frames: number[] = [];
  for (const p of parts) {
    const r = parseInteger(p, 0, count - 1);
    if (!r.ok) return { ok: false, error: `帧号${r.error}` };
    frames.push(r.value);
  }
  return { ok: true, value: frames };
}
