// 大地图：地形、场景、点地面挖一格。
import { expect, test } from "nsh/test";
import { BIG_CHUNK, BIG_COLS, BIG_LEVEL, BIG_MAP, BIG_ROWS, BIG_SPAWN, bigData, bigScene, digOf, groundRows } from "./bigmap";
import { TILE } from "./levels";

test("2000 × 80 格，两头是墙，每列地面都在地图里，同一份数据只算一次", () => {
  const data = bigData();
  expect(data.length).toBe(BIG_COLS * BIG_ROWS);
  expect(data).toBe(bigData());
  expect(data[0] >= 0 && data[BIG_COLS - 1] >= 0).toBe(true);
  for (let c = 1; c < BIG_COLS - 1; c++) {
    const rows = groundRows(c);
    expect(rows > 2 && rows < BIG_ROWS - 8).toBe(true);
    expect(data[(BIG_ROWS - 1) * BIG_COLS + c] >= 0).toBe(true);
  }
});

test("场景：地图分块、点得到，开着面板和曲线，角色在出生点、站在地面上面", () => {
  const scene = bigScene({ id: "player", w: 24, h: 32 }, 1);
  expect(scene.level).toBe(BIG_LEVEL);
  expect(scene.perf).toEqual({ panel: true, graph: true });
  const map = scene.tilemaps![0];
  expect([map.id, map.chunk, map.tap, map.cols]).toEqual([BIG_MAP, BIG_CHUNK, true, BIG_COLS]);
  expect(map.data).toBe(bigData());
  const player = scene.sprites![0];
  expect([player.x, player.y]).toEqual(BIG_SPAWN);
  expect(BIG_SPAWN[1] > groundRows(5) * TILE).toBe(true);
  expect(scene.camera!.follow).toBe("player");
});

test("点中有东西的格子挖掉，记下原来的帧；空格、别的东西不算", () => {
  expect(digOf({ id: BIG_MAP, x: 0, y: 0, cell: [7, 70], frame: 0 })).toEqual([7, 70, 0]);
  expect(digOf({ id: BIG_MAP, x: 0, y: 0, cell: [7, 3], frame: -1 })).toBe(null);
  expect(digOf({ id: "player", x: 0, y: 0 })).toBe(null);
  expect(digOf({ id: BIG_MAP, x: 0, y: 0 })).toBe(null);
});
