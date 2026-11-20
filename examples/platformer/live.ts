// 试玩热重载（docs/editor-more.md）：编辑器里改了关卡，正在跑的这一关就地更新。
// 瓦片地图走 setTiles（docs/big-scene-perf.md）：声明里留着最初的 data，只发变了的格子，引擎只重建改到的那一块。
import type { TilemapNode, Tween } from "../../ts/nge";
import type { Built } from "./levels";

/** `declared`：场景里声明的地图（格子可能是旧的）；`shown`：引擎现在显示的（声明加上发过的 setTiles）。 */
export interface LiveMaps {
  declared: TilemapNode[];
  shown: TilemapNode[];
}

export interface TileEdit {
  id: string;
  tiles: [number, number, number][];
}

export function startLive(maps: TilemapNode[]): LiveMaps {
  return { declared: maps, shown: maps };
}

/** 除了格子以外的字段（含格子数）：一样时只改格子就行。 */
const shape = ({ data, ...rest }: TilemapNode) => JSON.stringify({ ...rest, cells: data.length });

/**
 * 新的一组地图 `next`：和引擎现在显示的比，同一位置、字段一样的只把变了的格子发出去（声明不动）；
 * 别的（字段变了、新加的层）声明新的，引擎整张重建。`started` 是假（关卡还没建好）时都声明新的。
 */
export function liveTiles(live: LiveMaps, next: TilemapNode[], started = true): { maps: LiveMaps; edits: TileEdit[] } {
  const edits: TileEdit[] = [];
  const declared = next.map((m, i) => {
    const shown = live.shown[i];
    if (!started || shown === undefined || live.declared[i] === undefined || shape(shown) !== shape(m)) return m;
    const tiles: [number, number, number][] = [];
    m.data.forEach((f, k) => {
      if (shown.data[k] !== f) tiles.push([k % m.cols, Math.floor(k / m.cols), f]);
    });
    if (tiles.length > 0) edits.push({ id: m.id, tiles });
    return live.declared[i];
  });
  return { maps: { declared, shown: next }, edits };
}

type Lift = Built["lifts"][number];
type Enemy = Built["enemies"][number];

/** 移动平台的补间：起点写明是出生点（重发时不从平台现在的位置算起）。 */
export function liftTweenOf({ node, to, duration }: Lift): Tween {
  const from = { ...(to.x === undefined ? {} : { x: node.x }), ...(to.y === undefined ? {} : { y: node.y }) };
  return { id: node.id, from, to, duration, ease: "easeInOut", repeat: -1, yoyo: true };
}

/** 巡逻的敌人的补间：从出生点走到 `to` 再回来。 */
export function enemyTweenOf({ id, x, to }: Enemy): Tween {
  return { id, from: { x }, to, duration: 1.5, ease: "linear", repeat: -1, yoyo: true };
}

/** 热重载的状态：地图（声明的、显示的）和上一次的关卡。 */
export interface Follow {
  maps: LiveMaps;
  built: Built;
}

export function startFollow(built: Built): Follow {
  return { maps: startLive(built.tilemaps), built };
}

/**
 * 关卡变成了 `next`：要发的 setTiles、要重发的补间（变了的平台和敌人；`started` 是假时不发，levelStart 时统一发）、
 * 场景里要声明的地图。
 */
export function followStep(f: Follow, next: Built, started: boolean): { follow: Follow; edits: TileEdit[]; tweens: Tween[]; tilemaps: TilemapNode[] } {
  const r = liveTiles(f.maps, next.tilemaps, started);
  const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
  const tweens = started
    ? [
        ...next.lifts.filter((l) => !same(l, f.built.lifts.find((o) => o.node.id === l.node.id))).map(liftTweenOf),
        ...next.enemies.filter((e) => !same(e, f.built.enemies.find((o) => o.id === e.id))).map(enemyTweenOf),
      ]
    : [];
  return { follow: { maps: r.maps, built: next }, edits: r.edits, tweens, tilemaps: r.maps.declared };
}

/** 整关重开（引擎按声明重建）：声明换成现在显示的，发过的 setTiles 不丢，之后按重建后的样子比。 */
export function restartFollow(f: Follow): Follow {
  return { ...f, maps: startLive(f.maps.shown) };
}
