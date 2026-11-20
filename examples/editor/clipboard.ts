// 复制粘贴（docs/editor.md）：复制是深拷贝（带着子孙），粘贴换新 id、名字加「副本」、偏移一下，父节点跟着换成新 id。
import type { Entity, LevelFile } from "../../ts/level";
import { withDescendants } from "./hierarchy";

const clone = (e: Entity): Entity => JSON.parse(JSON.stringify(e)) as Entity;

/** 按场景树的顺序复制选中的和它们的子孙。 */
export function copy(level: LevelFile, ids: string[]): Entity[] {
  const wanted = new Set(withDescendants(level, ids));
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

/**
 * 要粘贴进去的实体：新 id、名字加「副本」、挪 `offset`（路线点一起挪）。父节点也复制了的换成新 id；
 * 父节点没复制的，关卡里还有、而且 `same(id)` 说还是复制时那个，就留着（粘贴到同一个分组里），不然放到最外层
 * （id 会重用：删掉最大号以后新放的东西拿到同一个 id）。
 */
export function pasted(level: LevelFile, clip: Entity[], [dx, dy]: [number, number], same: (parent: string) => boolean = () => true): Entity[] {
  const ids = freshIds(level, clip.length);
  const renamed = new Map(clip.map((e, i) => [e.id, ids[i]]));
  const existing = new Set(level.entities.map((e) => e.id));
  return clip.map((e, i) => {
    const { parent, ...rest } = clone(e);
    const out: Entity = { ...rest, id: ids[i], x: e.x + dx, y: e.y + dy };
    if (e.name !== undefined && e.name !== "") out.name = `${e.name} 副本`;
    if (e.points !== undefined) out.points = e.points.map(([x, y]) => [x + dx, y + dy]);
    const p = parent === undefined ? undefined : (renamed.get(parent) ?? (existing.has(parent) && same(parent) ? parent : undefined));
    if (p !== undefined) out.parent = p;
    return out;
  });
}
