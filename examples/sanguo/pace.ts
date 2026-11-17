import { DT } from "./player";

/** 一帧最多补几步（卡了一下时不一口气跑太多）。 */
export const MAX_STEPS = 5;

/**
 * 固定步长推进：`acc` 是上一帧余下的时间，`elapsed` 是这一帧经过的秒数。
 * 引擎还没画出第一帧（`ready` 前，web 上正在加载）或者暂停时不推进、不攒时间。
 */
export function pace(acc: number, elapsed: number, run: { ready: boolean; paused: boolean }): { acc: number; steps: number } {
  if (!run.ready || run.paused) return { acc, steps: 0 };
  let left = Math.min(acc + elapsed, DT * MAX_STEPS);
  let steps = 0;
  while (left >= DT - 1e-9) {
    left -= DT;
    steps++;
  }
  return { acc: Math.max(left, 0), steps };
}
