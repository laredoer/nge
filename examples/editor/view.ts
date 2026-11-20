// 画布的视图：拖动平移、以指针为中心缩放（docs/editor.md）。
import { screenToWorld, type CameraView } from "../../ts/nge";

export const MIN_ZOOM = 0.25;
export const MAX_ZOOM = 4;

/** 手指在屏幕上挪了 (dx, dy)（y 朝下）：画面跟着手走。 */
export function pan(view: CameraView, dx: number, dy: number): CameraView {
  return { ...view, x: view.x - dx / view.zoom, y: view.y + dy / view.zoom };
}

/** 缩放 `factor` 倍，屏幕上 `at` 那一点下面的世界坐标不动。 */
export function zoomAt(view: CameraView, size: [number, number], at: [number, number], factor: number): CameraView {
  const zoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, view.zoom * factor));
  const [wx, wy] = screenToWorld(view, size, at);
  // 缩放后 at 处的世界坐标要还是 (wx, wy)
  return { zoom, x: wx - (at[0] - size[0] / 2) / zoom, y: wy + (at[1] - size[1] / 2) / zoom };
}

/** 双指捏合（`pinch` 手势，docs/touch-input.md）：以挪动前的两指中点缩放 `factor` 倍，再跟着中点平移 (dx, dy)。 */
export function pinchView(view: CameraView, size: [number, number], g: { x: number; y: number; factor: number; dx: number; dy: number }): CameraView {
  return pan(zoomAt(view, size, [g.x - g.dx, g.y - g.dy], g.factor), g.dx, g.dy);
}

/**
 * 拖到画布边上自动滚动：指针离边 `margin` 以内时这一下挪多少（屏幕 point，y 朝下，和指针挪的方向一样），越靠边越快，
 * 出了画布按最快算；不在边上是 [0, 0]。
 */
export function edgeScroll(size: [number, number], [px, py]: [number, number], margin = 32, speed = 12): [number, number] {
  const axis = (p: number, length: number) => {
    if (length <= 2 * margin) return 0;
    if (p < margin) return -speed * Math.min(1, (margin - p) / margin);
    if (p > length - margin) return speed * Math.min(1, (p - (length - margin)) / margin);
    return 0;
  };
  return [axis(px, size[0]), axis(py, size[1])];
}
