// 试玩热重载（docs/editor-more.md）：编辑器里改了关卡，正在跑的这一关就地更新。
// 瓦片地图走 setTiles（docs/big-scene-perf.md）：声明里留着最初的 data，只发变了的格子，引擎只重建改到的那一块。
import type { TilemapNode } from "../../ts/nge";

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
