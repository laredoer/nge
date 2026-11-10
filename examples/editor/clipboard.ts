// 复制粘贴（docs/editor.md）：复制是深拷贝，粘贴换新 id、名字加「副本」、偏移一下。
import type { Entity, LevelFile } from "../../ts/level";

const clone = (e: Entity): Entity => JSON.parse(JSON.stringify(e)) as Entity;

/** 按场景树的顺序复制选中的。 */
export function copy(level: LevelFile, ids: string[]): Entity[] {
  const wanted = new Set(ids);
  return level.entities.filter((e) => wanted.has(e.id)).map(clone);
}

/** `count` 个新 id：`e<n>`，一般比已有的 `e<数字>` 都大；编号大到算不准（2^53 附近）时从小的空号里找。保证都没用过。 */
export function freshIds(level: LevelFile, count: number): string[] {
  const used = new Set(level.entities.map((e) => e.id));
  const max = level.entities.reduce((m, e) => {
    const n = /^e(\d+)$/.test(e.id) ? Number(e.id.slice(1)) : 0;
    return Number.isSafeInteger(n) ? Math.max(m, n) : m;
  }, 0);
  let n = Number.isSafeInteger(max + count) ? max + 1 : 1;
  const out: string[] = [];
  while (out.length < count) {
    const id = `e${n++}`;
    if (!used.has(id)) out.push(id);
  }
  return out;
}

/** 新的 id（第 `skip + 1` 个）。 */
export function nextId(level: LevelFile, skip = 0): string {
  return freshIds(level, skip + 1)[skip];
}

/** 要粘贴进去的实体：新 id、名字加「副本」、挪 `offset`。 */
export function pasted(level: LevelFile, clip: Entity[], [dx, dy]: [number, number]): Entity[] {
  const ids = freshIds(level, clip.length);
  return clip.map((e, i) => {
    const out: Entity = { ...clone(e), id: ids[i], x: e.x + dx, y: e.y + dy };
    if (e.name !== undefined && e.name !== "") out.name = `${e.name} 副本`;
    return out;
  });
}
