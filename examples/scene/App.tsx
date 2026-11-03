import { mount, signal, Text, View } from "nsh";
import { gameView, type Scene, type SpriteNode } from "../../ts/nge";

const GameView = gameView();

const RING = 8;

function App() {
  const taps = signal(0);
  const fps = signal(0);
  const ship = signal({ x: 0, y: -200 });

  const scene = (): Scene => {
    const rocks: SpriteNode[] = Array.from({ length: RING }, (_, i) => {
      const a = (i / RING) * Math.PI * 2;
      return { id: `rock-${i}`, texture: "ships", frame: 4 + (i % 4), w: 40, x: Math.cos(a) * 140, y: Math.sin(a) * 140 + 60, spin: 45 + i * 10 };
    });
    return {
      background: "#0a0a20",
      sprites: [
        { id: "sun", color: "#ffcc33", w: 60, y: 60, spin: 30, z: 1 },
        ...rocks,
        { id: "ship", texture: "ships", frame: 0, w: 64, x: ship().x, y: ship().y, rotation: 180, z: 10, tap: true },
      ],
    };
  };

  return (
    <View class="flex-1 bg-black">
      <Text class="text-white p-4">
        点飞船 {taps()} 次 · {fps()} FPS
      </Text>
      <GameView
        class="flex-1"
        scene={scene()}
        onTap={() => {
          taps.set(taps() + 1);
          ship.set({ x: (Math.random() - 0.5) * 300, y: -250 + Math.random() * 120 });
        }}
        onFps={(s) => fps.set(s.fps)}
      />
    </View>
  );
}

mount(App);
