import { mount, signal, Text, View, type YNode } from "nsh";
import { gameView, tween, type Scene, type SpriteNode } from "../../ts/nge";

const GameView = gameView();

const RING = 8;

function App() {
  let game: YNode | undefined;
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
        点飞船 {taps()} 次 · 到达 {arrived()} 次 · {fps()} FPS
      </Text>
      <GameView
        class="flex-1"
        ref={(n: YNode) => (game = n)}
        scene={scene()}
        onTap={() => {
          taps.set(taps() + 1);
          const to = { x: (Math.random() - 0.5) * 300, y: -250 + Math.random() * 120 };
          tween(game, { id: "ship", to, duration: 0.6, ease: "easeOut", tag: "move" });
          // 写明起点，连点打断时也会回到原来大小
          tween(game, { id: "ship", from: { scale: 1 }, to: { scale: 1.4 }, duration: 0.15, ease: "easeOut", yoyo: true, repeat: 1 });
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
