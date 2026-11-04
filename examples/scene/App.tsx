import { mount, signal, Text, View, type YNode } from "nsh";
import { gameView, shake, tween, type Scene, type SpriteNode } from "../../ts/nge";

const GameView = gameView();

const RING = 8;
/** 方向键每次移动多远。 */
const STEP = 40;
const ARROWS: Record<string, [number, number]> = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, 1], ArrowDown: [0, -1] };

function App() {
  let game: YNode | undefined;
  // 飞船要去的位置（位置交给补间，不进 signal）
  const target = { x: 0, y: -200 };
  const move = (duration: number, tag: string) => tween(game, { id: "ship", to: { ...target }, duration, ease: "easeOut", tag });
  const taps = signal(0);
  const arrived = signal(0);
  const fps = signal(0);

  // 位置交给补间，TSX 只声明一次初始位置
  const scene = (): Scene => {
    const rocks: SpriteNode[] = Array.from({ length: RING }, (_, i) => {
      const a = (i / RING) * Math.PI * 2;
      const frames = Array.from({ length: 8 }, (_, k) => 4 + ((i + k) % 8));
      return { id: `rock-${i}`, texture: "ships", anim: { frames, fps: 4 }, w: 40, x: Math.cos(a) * 140, y: Math.sin(a) * 140 + 60, spin: 45 + i * 10 };
    });
    return {
      background: "#0a0a20",
      // 相机平滑地跟着飞船，飞船在画面偏下；走到边上不拍出这片区域
      camera: { follow: "ship", y: 120, smooth: 0.25, bounds: { left: -400, right: 400, bottom: -500, top: 500 } },
      sprites: [
        { id: "sun", color: "#ffcc33", w: 60, y: 60, spin: 30, z: 1 },
        ...rocks,
        { id: "ship", texture: "ships", frame: 0, w: 64, y: -200, rotation: 180, z: 10, tap: true },
      ],
    };
  };

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        点飞船 {taps()} 次 · 方向键移动 · 到达 {arrived()} 次 · {fps()} FPS
      </Text>
      <GameView
        class="flex-1"
        ref={(n: YNode) => (game = n)}
        scene={scene()}
        onTap={() => {
          taps.set(taps() + 1);
          target.x = (Math.random() - 0.5) * 300;
          target.y = -250 + Math.random() * 120;
          move(0.6, "move");
          // 写明起点，连点打断时也会回到原来大小
          tween(game, { id: "ship", from: { scale: 1 }, to: { scale: 1.4 }, duration: 0.15, ease: "easeOut", yoyo: true, repeat: 1 });
          shake(game, { strength: 6, duration: 0.25 });
        }}
        onKey={(e) => {
          const dir = ARROWS[e.code];
          if (!e.down || !dir) return;
          target.x = Math.max(-180, Math.min(180, target.x + dir[0] * STEP));
          target.y = Math.max(-300, Math.min(300, target.y + dir[1] * STEP));
          // 按住时系统自动重复，飞船连续移动；不算「到达」
          move(0.15, "nudge");
        }}
        onTweenEnd={(e) => {
          // 连点时上一次移动被接管（cancelled），没有到达
          if (e.tag === "move" && !e.cancelled) arrived.set(arrived() + 1);
        }}
        onFps={(s) => fps.set(s.fps)}
      />
    </View>
  );
}

mount(App);
