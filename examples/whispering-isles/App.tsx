import { mount, type NativeComponentRef, safeArea, signal, Text, View } from "nsh";
import { camera3dTrack, gameView, saved, shake, sound, type Pick3d } from "../../ts/nge";
import { ATMOSPHERES, DEFAULT_QUALITY, RELICS, regionOf, SHRINES, SOUNDS, worldScene, type Fx3dPreset, type QualitySettings, type ShadingPreset, type ShrineInfo, type TimeOfDay, type ToneMapOption } from "./scene";
import "whispering-isles/native";

interface Telemetry {
  x: number;
  y: number;
  z: number;
  yaw: number;
  cam_yaw: number;
  stamina: number;
  max_stamina: number;
  gliding: boolean;
  grounded: boolean;
}

interface Interact {
  x: number;
  y: number;
  z: number;
}

const GameView = gameView<{}, { telemetry: Telemetry; interact: Interact }>({
  events: ["telemetry", "interact"],
});

const CAPTURE_KEYS = [
  "Space", "ShiftLeft", "ShiftRight", "Enter", "Escape",
  "KeyW", "KeyA", "KeyS", "KeyD", "KeyQ", "KeyE", "KeyJ", "KeyL",
  "KeyF", "KeyT", "KeyV", "KeyC", "KeyP", "KeyR", "KeyX", "KeyO",
  "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
];

function wrapAngle(a: number): number {
  let v = a;
  while (v > Math.PI) v -= Math.PI * 2;
  while (v < -Math.PI) v += Math.PI * 2;
  return v;
}

function compassDirection(camYaw: number): string {
  // cam_yaw = 0 时面向 -Z（正北）
  const deg = (((-camYaw * 180) / Math.PI) % 360 + 360) % 360;
  const dirs = ["北 N", "东北 NE", "东 E", "东南 SE", "南 S", "西南 SW", "西 W", "西北 NW"];
  return dirs[Math.round(deg / 45) % 8];
}

function App() {
  let gameRef: NativeComponentRef | undefined;
  const awakened = saved<string[]>("whispering-isles:shrines", []);
  const collected = saved<string[]>("whispering-isles:relics", []);
  const tod = signal<TimeOfDay>(0);
  const round = signal(1);
  const quality = signal<QualitySettings>({ ...DEFAULT_QUALITY });
  const beacon = signal<[number, number, number] | null>(null);
  const toast = signal<{ title: string; body: string } | null>(null);
  const pos = signal<Telemetry>({
    x: 0,
    y: 56,
    z: 285,
    yaw: 0,
    cam_yaw: 0,
    stamina: 100,
    max_stamina: 100,
    gliding: false,
    grounded: true,
  });

  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  const showToast = (title: string, body: string, ms = 4200) => {
    toast.set({ title, body });
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.set(null), ms);
  };

  const nearbyShrine = (): ShrineInfo | null => {
    const p = pos();
    let best: ShrineInfo | null = null;
    let bestDist = 115;
    for (const s of SHRINES) {
      const d = Math.hypot(s.x - p.x, s.z - p.z);
      if (d < bestDist && Math.abs(s.y - p.y) < 95) {
        bestDist = d;
        best = s;
      }
    }
    return best;
  };

  const handleTelemetry = (t: Telemetry) => {
    pos.set(t);
    // 自动拾取靠近的风之微光
    const cur = collected();
    for (const r of RELICS) {
      if (cur.includes(r.id)) continue;
      const d = Math.hypot(r.x - t.x, r.z - t.z);
      if (d < 52 && Math.abs(r.y - t.y) < 58) {
        const next = [...cur, r.id];
        collected.set(next);
        sound(gameRef, { src: SOUNDS.relic, volume: 0.65 });
        showToast(`拾取风之微光 (${next.length}/${RELICS.length})`, "清风环绕周身，探索旅途更加轻盈。", 2800);
        break;
      }
    }
  };

  let beaconTimer: ReturnType<typeof setTimeout> | undefined;
  const dropBeacon = (pt: [number, number, number], ms = 7000) => {
    beacon.set(pt);
    if (beaconTimer) clearTimeout(beaconTimer);
    beaconTimer = setTimeout(() => beacon.set(null), ms);
  };

  const interactShrine = (s: ShrineInfo) => {
    const cur = awakened();
    if (cur.includes(s.id)) {
      showToast(s.name, s.lore, 4500);
      return;
    }
    const next = [...cur, s.id];
    awakened.set(next);
    sound(gameRef, { src: SOUNDS.shrine, volume: 0.75 });
    shake(gameRef, { strength: 14, duration: 0.45 });
    camera3dTrack(gameRef, {
      tag: `shrine:${s.id}`,
      returnDuration: 0.9,
      keyframes: [
        { pos: [s.x + 150, s.y + 95, s.z + 150], target: [s.x, s.y + 72, s.z], duration: 0.9, fov: 48, ease: "easeOut" },
        { pos: [s.x - 165, s.y + 135, s.z + 120], target: [s.x, s.y + 96, s.z], duration: 1.5, fov: 50, ease: "easeInOut" },
      ],
    });
    const title = next.length === SHRINES.length ? `全部圣坛已共鸣 · ${s.name}` : `唤醒圣坛 · ${s.name} (${next.length}/${SHRINES.length})`;
    showToast(title, s.lore, 5500);
  };

  const playFlyover = () => {
    sound(gameRef, { src: SOUNDS.time, volume: 0.4 });
    showToast("3D 过场运镜预览", "正在沿关键帧轨道巡游风语群岛（按 Esc 可随时跳过）", 4500);
    camera3dTrack(gameRef, {
      tag: "flyover",
      returnDuration: 1.0,
      keyframes: [
        { pos: [190, 140, 260], target: [0, 42, 160], duration: 1.1, fov: 52, ease: "easeInOut" },
        { pos: [260, 220, -160], target: [0, 120, -480], duration: 1.8, fov: 54, ease: "easeInOut" },
        { pos: [-210, 310, -520], target: [0, 220, -760], duration: 1.7, fov: 48, ease: "easeInOut" },
      ],
    });
  };

  const handleInteract = () => {
    const s = nearbyShrine();
    if (s) interactShrine(s);
  };

  const handlePick3d = (e: Pick3d) => {
    const shrine = e.id ? SHRINES.find((s) => s.id === e.id) : undefined;
    if (shrine) {
      const p = pos();
      const dist = Math.round(Math.hypot(shrine.x - p.x, shrine.z - p.z));
      dropBeacon([shrine.x, shrine.y, shrine.z]);
      if (dist <= 260 || awakened().includes(shrine.id)) {
        interactShrine(shrine);
      } else {
        sound(gameRef, { src: SOUNDS.time, volume: 0.35 });
        showToast(`远眺圣坛 · ${shrine.name}（距 ${dist}m）`, "已派风灵飞往圣坛标点，靠近后可唤醒或点击互动。", 3200);
      }
      return;
    }
    dropBeacon(e.point);
    sound(gameRef, { src: SOUNDS.time, volume: 0.25 });
    const targetName = e.id && e.id !== "terrain" ? `「${e.id}」` : "地面标点";
    showToast("风灵前往侦察", `已点击 ${targetName} (${Math.round(e.point[0])}, ${Math.round(e.point[1])}, ${Math.round(e.point[2])})`, 2200);
  };

  const cycleTime = () => {
    const next = ((tod() + 1) % 4) as TimeOfDay;
    tod.set(next);
    sound(gameRef, { src: SOUNDS.time, volume: 0.45 });
  };

  const resetProgress = () => {
    awakened.set([]);
    collected.set([]);
    round.set(round() + 1);
    showToast("新的旅途", "已重置风语群岛的圣坛与风之微光。", 2600);
  };

  const cycleToneMap = () => {
    const order: ToneMapOption[] = ["aces", "reinhard", "none"];
    const cur = quality();
    const next = order[(order.indexOf(cur.tonemap) + 1) % order.length];
    quality.set({ ...cur, tonemap: next });
  };
  const cycleShading = () => {
    const order: ShadingPreset[] = ["stylized", "toon", "raw"];
    const cur = quality();
    const next = order[(order.indexOf(cur.shading) + 1) % order.length];
    quality.set({ ...cur, shading: next });
  };
  const cycleFx3d = () => {
    const order: Fx3dPreset[] = ["aura", "holo", "off"];
    const cur = quality();
    const next = order[(order.indexOf(cur.fx3d) + 1) % order.length];
    quality.set({ ...cur, fx3d: next });
  };
  const toggleFlag = (key: "ssao" | "fxaa" | "bloom" | "navDebug" | "perf") => {
    const cur = quality();
    quality.set({ ...cur, [key]: !cur[key] });
  };

  return (
    <View class="flex-1 bg-black">
      <GameView
        class="flex-1"
        ref={(n: NativeComponentRef) => (gameRef = n)}
        captureKeys={CAPTURE_KEYS}
        scene={worldScene(new Set(awakened()), new Set(collected()), tod(), round(), quality(), beacon())}
        onTelemetry={handleTelemetry}
        onInteract={handleInteract}
        onPick3d={handlePick3d}
        onKey={(e) => {
          if (!e.down || e.repeat) return;
          if (e.code === "KeyT") cycleTime();
          else if (e.code === "KeyP") toggleFlag("perf");
          else if (e.code === "KeyR") round.set(round() + 1);
          else if (e.code === "KeyX") shake(gameRef, { strength: 16, duration: 0.45 });
          else if (e.code === "KeyO") playFlyover();
          else if (e.code === "Escape") camera3dTrack(gameRef, { stop: true });
        }}
      />
      <Hud
        pos={pos}
        awakened={awakened}
        collected={collected}
        tod={tod}
        quality={quality}
        nearby={nearbyShrine}
        toast={toast}
        onCycleTime={cycleTime}
        onCycleToneMap={cycleToneMap}
        onCycleShading={cycleShading}
        onCycleFx3d={cycleFx3d}
        onToggleFlag={toggleFlag}
        onShake={() => shake(gameRef, { strength: 16, duration: 0.45 })}
        onFlyover={playFlyover}
        onReset={resetProgress}
      />
    </View>
  );
}

function Hud(props: {
  pos: () => Telemetry;
  awakened: () => string[];
  collected: () => string[];
  tod: () => TimeOfDay;
  quality: () => QualitySettings;
  nearby: () => ShrineInfo | null;
  toast: () => { title: string; body: string } | null;
  onCycleTime: () => void;
  onCycleToneMap: () => void;
  onCycleShading: () => void;
  onCycleFx3d: () => void;
  onToggleFlag: (key: "ssao" | "fxaa" | "bloom" | "navDebug" | "perf") => void;
  onShake: () => void;
  onFlyover: () => void;
  onReset: () => void;
}) {
  const p = props.pos;
  const region = () => regionOf(p().x, p().z);
  const atmo = () => ATMOSPHERES[props.tod()];
  const awakeSet = () => new Set(props.awakened());
  const staminaPct = () => Math.max(0, Math.min(100, Math.round((p().stamina / p().max_stamina) * 100)));

  // 把世界坐标映射到 128×128 小地图
  const mapCoord = (wx: number, wz: number): [number, number] => [
    Math.max(4, Math.min(120, Math.round(64 + (wx / 1200) * 56))),
    Math.max(4, Math.min(120, Math.round(64 + (wz / 1200) * 56))),
  ];

  return (
    <View style={{ pointerEvents: "box-none", top: safeArea().top, left: safeArea().left, right: safeArea().right, bottom: safeArea().bottom }} class="absolute">
      {/* 顶部中央：天空指南针（标示朝向与未唤醒圣坛方向） */}
      <View class="absolute top-3 left-0 right-0 items-center" style={{ pointerEvents: "none" }}>
        <View class="w-80 h-9 bg-black/55 rounded-full items-center justify-center overflow-hidden" style={{ borderWidth: 1, borderColor: "#ffffff24" }}>
          <Text class="text-white/90 text-xs font-bold">{compassDirection(p().cam_yaw)}</Text>
          {SHRINES.map((s) => {
            const dx = s.x - p().x;
            const dz = s.z - p().z;
            const targetYaw = Math.atan2(-dx, -dz);
            const rel = wrapAngle(targetYaw - p().cam_yaw);
            if (Math.abs(rel) > 1.25) return null;
            const left = Math.round(160 - (rel / 1.25) * 140) - 5;
            const done = awakeSet().has(s.id);
            return (
              <View
                class="absolute top-1.5 w-2.5 h-2.5 rounded-full"
                style={{ left, backgroundColor: done ? "#38e8ff" : "#fbbf24" }}
              />
            );
          })}
        </View>
      </View>

      {/* 左侧中部：区域名、昼夜与实时 3D 画质对比面板（避开左上角 perf 面板） */}
      <View class="absolute left-4 bg-black/60 px-4 py-3 rounded-2xl gap-1.5" style={{ top: props.quality().perf ? 116 : 16, borderWidth: 1, borderColor: "#ffffff1e" }}>
        <Text class="text-amber-200 text-base font-bold">{region()}</Text>
        <View class="flex-row items-center gap-3">
          <Text class="text-cyan-300 text-xs font-bold">{`圣坛 ${props.awakened().length}/${SHRINES.length}`}</Text>
          <Text class="text-amber-300 text-xs font-bold">{`微光 ${props.collected().length}/${RELICS.length}`}</Text>
        </View>
        <View class="flex-row items-center gap-2 mt-1">
          <View class="px-2.5 py-1 rounded-lg bg-white/15" onPress={props.onCycleTime}>
            <Text class="text-white text-xs">{`天色：${atmo().label} (T)`}</Text>
          </View>
          {props.awakened().length > 0 || props.collected().length > 0 ? (
            <View class="px-2.5 py-1 rounded-lg bg-white/10" onPress={props.onReset}>
              <Text class="text-white/70 text-xs">重置进度</Text>
            </View>
          ) : null}
        </View>
        {/* 实时画质对比开关 */}
        <View class="mt-1.5 pt-1.5 gap-1.5" style={{ borderTopWidth: 1, borderTopColor: "#ffffff18" }}>
          <Text class="text-white/60 text-xs">3D 画质实时对比</Text>
          <View class="flex-row items-center gap-1.5">
            <View class="px-2 py-1 rounded-lg bg-sky-500/30" onPress={props.onCycleToneMap}>
              <Text class="text-sky-100 text-xs">{`色调：${props.quality().tonemap.toUpperCase()}`}</Text>
            </View>
            <View class="px-2 py-1 rounded-lg bg-emerald-500/30" onPress={props.onCycleShading}>
              <Text class="text-emerald-100 text-xs">
                {`材质：${props.quality().shading === "stylized" ? "水+风吹" : props.quality().shading === "toon" ? "卡通 Toon" : "原版 PBR"}`}
              </Text>
            </View>
          </View>
          <View class="flex-row items-center gap-1.5">
            <View class="px-2 py-1 rounded-lg bg-violet-500/35" onPress={props.onCycleFx3d}>
              <Text class="text-violet-100 text-xs">
                {`3D Shader：${props.quality().fx3d === "aura" ? "风灵护盾" : props.quality().fx3d === "holo" ? "全息扫描" : "关闭"}`}
              </Text>
            </View>
            <View class={`px-2 py-1 rounded-lg ${props.quality().navDebug ? "bg-cyan-500/35" : "bg-white/10"}`} onPress={() => props.onToggleFlag("navDebug")}>
              <Text class="text-white text-xs">{`导航线:${props.quality().navDebug ? "开" : "关"}`}</Text>
            </View>
            <View class="px-2 py-1 rounded-lg bg-rose-500/35" onPress={props.onShake}>
              <Text class="text-rose-100 text-xs">震屏(X)</Text>
            </View>
            <View class="px-2 py-1 rounded-lg bg-indigo-500/40" onPress={props.onFlyover}>
              <Text class="text-indigo-100 text-xs">运镜(O)</Text>
            </View>
          </View>
          <View class="flex-row items-center gap-1.5">
            <View class={`px-2 py-1 rounded-lg ${props.quality().ssao ? "bg-amber-500/35" : "bg-white/10"}`} onPress={() => props.onToggleFlag("ssao")}>
              <Text class="text-white text-xs">{`SSAO:${props.quality().ssao ? "开" : "关"}`}</Text>
            </View>
            <View class={`px-2 py-1 rounded-lg ${props.quality().fxaa ? "bg-amber-500/35" : "bg-white/10"}`} onPress={() => props.onToggleFlag("fxaa")}>
              <Text class="text-white text-xs">{`FXAA:${props.quality().fxaa ? "开" : "关"}`}</Text>
            </View>
            <View class={`px-2 py-1 rounded-lg ${props.quality().bloom ? "bg-amber-500/35" : "bg-white/10"}`} onPress={() => props.onToggleFlag("bloom")}>
              <Text class="text-white text-xs">{`Bloom:${props.quality().bloom ? "开" : "关"}`}</Text>
            </View>
            <View class={`px-2 py-1 rounded-lg ${props.quality().perf ? "bg-cyan-500/35" : "bg-white/10"}`} onPress={() => props.onToggleFlag("perf")}>
              <Text class="text-white text-xs">统计(P)</Text>
            </View>
          </View>
        </View>
      </View>

      {/* 右上角：群岛实时小地图 */}
      <View class="absolute top-4 right-4 items-center gap-1" style={{ pointerEvents: "none" }}>
        <View class="w-32 h-32 rounded-2xl bg-sky-950/75 overflow-hidden" style={{ borderWidth: 1.5, borderColor: "#ffffff28" }}>
          {/* 岛屿轮廓 */}
          <View class="absolute rounded-full bg-emerald-800/75" style={{ left: 16, top: 16, width: 96, height: 96 }} />
          {/* 5 座圣坛点 */}
          {SHRINES.map((s) => {
            const [mx, my] = mapCoord(s.x, s.z);
            const done = awakeSet().has(s.id);
            return (
              <View
                class="absolute w-2.5 h-2.5 rounded-full"
                style={{ left: mx - 5, top: my - 5, backgroundColor: done ? "#38e8ff" : "#fbbf24" }}
              />
            );
          })}
          {/* 玩家当前位置 */}
          {(() => {
            const [px, py] = mapCoord(p().x, p().z);
            return <View class="absolute w-3 h-3 rounded-full bg-white" style={{ left: px - 6, top: py - 6, borderWidth: 1.5, borderColor: "#0ea5e9" }} />;
          })()}
        </View>
        <Text class="text-white/65 text-xs">{p().gliding ? "滑翔中" : `海拔 ${Math.round(p().y)}m`}</Text>
      </View>

      {/* 探索发现 / 圣坛碑文横幅 */}
      {props.toast() ? (
        <View class="absolute top-20 left-0 right-0 items-center px-8" style={{ pointerEvents: "none" }}>
          <View class="max-w-lg bg-black/75 px-6 py-3.5 rounded-2xl items-center gap-1" style={{ borderWidth: 1, borderColor: "#fbbf2455" }}>
            <Text class="text-amber-300 text-lg font-bold">{props.toast()!.title}</Text>
            <Text class="text-white/90 text-sm">{props.toast()!.body}</Text>
          </View>
        </View>
      ) : null}

      {/* 靠近圣坛时的交互提示 */}
      {props.nearby() ? (
        <View class="absolute bottom-20 left-0 right-0 items-center" style={{ pointerEvents: "none" }}>
          <View class="bg-black/75 px-5 py-2 rounded-full" style={{ borderWidth: 1, borderColor: "#38e8ff80" }}>
            <Text class="text-cyan-200 text-sm font-bold">
              {awakeSet().has(props.nearby()!.id) ? `[F] 阅读碑文 · ${props.nearby()!.name}` : `[F] 唤醒圣坛 · ${props.nearby()!.name}`}
            </Text>
          </View>
        </View>
      ) : null}

      {/* 冲刺 / 滑翔体力条（非满时显示） */}
      {staminaPct() < 99 || p().gliding ? (
        <View class="absolute bottom-11 left-0 right-0 items-center" style={{ pointerEvents: "none" }}>
          <View class="w-44 h-2.5 bg-black/60 rounded-full overflow-hidden" style={{ borderWidth: 1, borderColor: "#ffffff30" }}>
            <View class="h-2.5 rounded-full" style={{ width: `${staminaPct()}%`, backgroundColor: p().gliding ? "#38e8ff" : "#4ade80" }} />
          </View>
        </View>
      ) : null}

      {/* 底部操作说明 */}
      <View class="absolute bottom-3 left-0 right-0 items-center" style={{ pointerEvents: "none" }}>
        <Text class="text-white/65 text-xs">WASD 移动 · Q/E 旋转视角 · Space 跳跃/滑翔 · 左键点击地面引导风灵/点击圣坛互动 · F 交互 · T 昼夜 · V 视角</Text>
      </View>
    </View>
  );
}

mount(App);
