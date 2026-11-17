// Mac 触控板的捏合（docs/touch-input.md）。
import type { Accessor } from "nsh";
import type { CameraView } from "../../ts/nge";
import { zoomAt } from "./view";

/** 定时器：测试里换成手动的。 */
export interface Ticker {
  every(fn: () => void, ms: number): number;
  stop(handle: number | undefined): void;
}

const SYSTEM: Ticker = { every: (fn, ms) => setInterval(fn, ms), stop: (h) => clearInterval(h) };

/**
 * 触控板的捏合（和 Control + 滚轮）：nsh 只在开始、结束时告诉 JS，中间的倍数在 `pinch` 的动画值（`scale`）里，
 * 捏的时候每 33 ms 读一次，以鼠标最后在画布上的位置为中心缩放。读数是异步的：每次开始、结束、停掉换一个序号，
 * 晚到的（上一次捏的、停掉以后的）不算；读失败不管。`stop`：卸载时（⌘R 试玩）停掉计时器。
 */
export function trackpadZoom(
  s: { view: Accessor<CameraView> & { set(v: CameraView): void }; size: () => [number, number] },
  scale: { getValue(): Promise<number>; set(v: number): void },
  ticker: Ticker = SYSTEM,
) {
  let base = s.view();
  let center: [number, number] = [0, 0];
  let timer: number | undefined;
  let round = 0;
  const apply = (v: number) => s.view.set(zoomAt(base, s.size(), center, v));
  const t = {
    /** 鼠标最后在画布上的位置；没进过画布时用中心。 */
    mouse: null as [number, number] | null,
    start() {
      base = s.view();
      center = t.mouse ?? [s.size()[0] / 2, s.size()[1] / 2];
      t.stop();
      const mine = round;
      timer = ticker.every(() => {
        scale.getValue().then(
          (v) => mine === round && apply(v),
          () => {},
        );
      }, 33);
    },
    end(e: { scale: number }) {
      t.stop();
      apply(e.scale);
      scale.set(1);
    },
    stop() {
      round++;
      ticker.stop(timer);
      timer = undefined;
    },
  };
  return t;
}
