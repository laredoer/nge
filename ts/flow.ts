// 预制体、预加载、换关过场（见 docs/scene-flow.md）：场景里的类型和接引擎事件的帮手。
import { signal, type NativeComponentRef, type Signal } from "nsh";
import type { BarNode, BubbleNode, EmitterNode, JointNode, SpriteNode, TextNode, TilemapNode } from "./nge";

/** 一组可复用的节点。子节点的 id 在实例里变成 `实例 id/子节点 id`。 */
export interface Prefab {
  sprites?: SpriteNode[];
  tilemaps?: TilemapNode[];
  texts?: TextNode[];
  emitters?: EmitterNode[];
  bars?: BarNode[];
  bubbles?: BubbleNode[];
  joints?: JointNode[];
  /** 嵌套的实例。 */
  instances?: PrefabInstance[];
}

/** 预制体的一个实例。 */
export interface PrefabInstance {
  id: string;
  /** `scene.prefabs` 里的名字，或 `asset:/` 的 JSON 文件。 */
  prefab: string;
  x?: number;
  y?: number;
  z?: number;
  /** 按子节点 id 覆盖字段（浅合并，坐标是预制体里的）；嵌套的写 `a/crown`。 */
  set?: Record<string, Record<string, unknown>>;
}

/** 换关过场：`level` 换了值时先盖上、预加载新关卡，好了再揭开；过场里游戏收不到输入。 */
export interface TransitionNode {
  /** 默认 "fade"：整屏变成 `color`；"wipe" 从左往右盖上再揭开。 */
  kind?: "fade" | "wipe";
  /** 默认 "#000000"。 */
  color?: string;
  /** 盖上用几秒，默认 0.3（真实时间）。 */
  out?: number;
  /** 揭开用几秒，默认 0.3。 */
  in?: number;
  /** 从开始盖到换关最少几秒，默认 0。 */
  min?: number;
  /** 另外要等的资源（场景里看不出来的，比如只在命令里播的音效）。 */
  preload?: string[];
}

/** `scene.textures` 的一项。 */
export interface TextureOptions {
  /** 不写按图片自己的（PNG / JPEG 生成）；像素风、UI 关掉。 */
  mipmaps?: boolean;
}

export interface LoadFailure {
  uri: string;
  error: string;
}

/** 引擎发来的 `preload` 事件。失败的也算 `done`。 */
export interface PreloadEvent {
  id: string;
  done: number;
  total: number;
  failed: LoadFailure[];
  finished: boolean;
}

/** 引擎发来的 `transition` 事件：`out` 开始盖（之后加载进度变了再发）、`in` 换好了开始揭开、`done` 揭开了。 */
export interface TransitionEvent {
  phase: "out" | "in" | "done";
  level: string | null;
  done: number;
  total: number;
  failed: LoadFailure[];
}

export interface Loading {
  id: string;
  /** 最新的进度，引擎还没回时是 undefined。加载 JSON 文件后总数会变大，`done / total` 可能变小。 */
  progress: Signal<PreloadEvent | undefined>;
  /** 进度条用的比例 0 … 1：只涨不退。 */
  fraction(): number;
  /** 全部完成（失败的也算）时 resolve。 */
  ready: Promise<PreloadEvent>;
}

export interface Preloader {
  /** 开一组预加载。视图还没建好时直接完成（total 0）。 */
  load(uris: string[]): Loading;
  /** 接 `<GameView onPreload={loads.handle}>`。 */
  handle(event: PreloadEvent): void;
  /** 还没完成的组数。 */
  pending(): number;
  /** 视图重建了（之前的命令引擎收不到了）：还没完成的组都按「没回」结束。 */
  reset(): void;
}

/** 引擎多久没回进度就按「没回」结束（毫秒）。 */
const WAIT_MS = 30000;

/** 所有预加载器共用的编号：几个接同一个事件时不会串。 */
let nextId = 1;

/** `game` 拿游戏视图（`<GameView ref>`）。引擎 `waitMs` 毫秒没回进度时这一组按「没回」结束，免得一直挂着。 */
export function preloader(game: () => NativeComponentRef | undefined, settings: { waitMs?: number } = {}): Preloader {
  type Entry = { uris: string[]; progress: Signal<PreloadEvent | undefined>; resolve: (e: PreloadEvent) => void; timer: ReturnType<typeof setTimeout> };
  const waiting = new Map<string, Entry>();
  const finish = (id: string, event: PreloadEvent) => {
    const entry = waiting.get(id);
    if (entry === undefined) return;
    waiting.delete(id);
    clearTimeout(entry.timer);
    entry.progress.set(event);
    entry.resolve(event);
  };
  // 没回：已经报失败的留着，别的不知道载好没有，都记成失败
  const giveUp = (id: string) => {
    const entry = waiting.get(id)!;
    const last = entry.progress();
    const failed = [...(last?.failed ?? [])];
    for (const uri of entry.uris) if (!failed.some((f) => f.uri === uri)) failed.push({ uri, error: "引擎没回进度，不知道载好没有" });
    const total = Math.max(last?.total ?? 0, entry.uris.length);
    finish(id, { id, done: total, total, failed, finished: true });
  };
  const arm = (id: string) => setTimeout(() => waiting.has(id) && giveUp(id), settings.waitMs ?? WAIT_MS);
  return {
    load(uris) {
      const id = `preload-${nextId++}`;
      const progress = signal<PreloadEvent | undefined>(undefined);
      let best = 0;
      const fraction = () => {
        const p = progress();
        const now = p === undefined ? 0 : p.finished || p.total === 0 ? 1 : p.done / p.total;
        best = Math.max(best, now);
        return best;
      };
      const view = game();
      if (view === undefined) {
        const empty: PreloadEvent = { id, done: 0, total: 0, failed: [], finished: true };
        progress.set(empty);
        return { id, progress, fraction, ready: Promise.resolve(empty) };
      }
      const ready = new Promise<PreloadEvent>((resolve) => waiting.set(id, { uris, progress, resolve, timer: arm(id) }));
      view.command("preload", { id, uris });
      return { id, progress, fraction, ready };
    },
    handle(event) {
      const entry = waiting.get(event.id);
      if (entry === undefined) return;
      if (event.finished) return finish(event.id, event);
      entry.progress.set(event);
      clearTimeout(entry.timer);
      entry.timer = arm(event.id);
    },
    pending: () => waiting.size,
    reset() {
      for (const id of [...waiting.keys()]) giveUp(id);
    },
  };
}

export interface Transitions {
  /** 正在过场（盖上、等资源、揭开）。 */
  busy(): boolean;
  /** 最近一次的阶段，没过过场是 undefined。 */
  phase(): TransitionEvent["phase"] | undefined;
  /** 过场里的加载进度。 */
  progress(): { done: number; total: number };
  /** 过场里有东西在载（显示「载入中」用）：开场的揭开、没有资源要等的过场是 false。 */
  loading(): boolean;
  /** 接 `<GameView onTransition={fade.handle}>`。 */
  handle(event: TransitionEvent): void;
}

export function transitions(): Transitions {
  const last = signal<TransitionEvent | undefined>(undefined);
  return {
    busy: () => {
      const phase = last()?.phase;
      return phase === "out" || phase === "in";
    },
    phase: () => last()?.phase,
    loading: () => {
      const e = last();
      return e !== undefined && e.phase !== "done" && e.total > 0;
    },
    progress: () => {
      const e = last();
      return { done: e?.done ?? 0, total: e?.total ?? 0 };
    },
    handle(event) {
      // 揭开时引擎不带进度：留着淡出时的
      const before = last();
      last.set(event.phase === "out" || before === undefined ? event : { ...event, done: before.done, total: before.total, failed: before.failed });
    },
  };
}
