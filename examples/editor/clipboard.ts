// 复制粘贴（docs/editor.md）：复制是深拷贝，粘贴换新 id、名字加「副本」、偏移一下。
import type { Entity, LevelFile } from "../../ts/level";

const clone = (e: Entity): Entity => JSON.parse(JSON.stringify(e)) as Entity;

/** 按场景树的顺序复制选中的。 */
export function copy(level: LevelFile, ids: string[]): Entity[] {
  const wanted = new Set(ids);
  return level.entities.filter((e) => wanted.has(e.id)).map(clone);
}

/** 新的 id：`e<n>`，n 比关卡里已有的 `e<数字>` 都大。 */
export function nextId(level: LevelFile, skip = 0): string {
  const max = level.entities.reduce((m, e) => Math.max(m, /^e(\d+)$/.test(e.id) ? Number(e.id.slice(1)) : 0), 0);
  return `e${max + 1 + skip}`;
}

/** 要粘贴进去的实体：新 id、名字加「副本」、挪 `offset`。 */
export function pasted(level: LevelFile, clip: Entity[], [dx, dy]: [number, number]): Entity[] {
  return clip.map((e, i) => {
    const out: Entity = { ...clone(e), id: nextId(level, i), x: e.x + dx, y: e.y + dy };
    if (e.name !== undefined && e.name !== "") out.name = `${e.name} 副本`;
    return out;
  });
}
