// SPDX-License-Identifier: GPL-3.0-or-later
// Test support (not built): a timeline world for the interaction machine. The machine runs with
// the package's real hit-testing (`hitTest` over a `TimelineLayout`) and the real shared snapping
// (the wasm math module), behind a host that applies view intents the way the editor will
// (scrolling, zooming), and a fake engine that records commands and checks the transaction
// protocol: a gesture opens once, updates only while open, and closes exactly once.
import { hitTest } from "../../hit-test.ts";
import { type ItemLanes, laneRowHeight } from "../../keyframes/lanes.ts";
import { TimelineLayout } from "../../layout.ts";
import type { Rect, TimelineItem, TimelineModel, TimelineTrack } from "../../model.ts";
import { testTheme } from "../../test-support.ts";
import { FLICKS_PER_SECOND } from "../../time.ts";
import { type Viewport, xToTime } from "../../viewport.ts";
import { type ChartEdge, type MachineTransition, transition } from "../reducer.ts";
import { createSnapPort, type WasmSnapPort } from "../snap-port.ts";
import { initialMachineState, type MachineState } from "../state.ts";
import type {
  DroppedMedia,
  KeyframeDragHandler,
  MachineCommand,
  MachineDeps,
  MachineEnv,
  MachineEvent,
  MachineIntent,
} from "../types.ts";

/** The fixtures run at 25 fps, like the spec's diagrams. */
export const FPS = { num: 25, den: 1 } as const;
export const FRAME = FLICKS_PER_SECOND / 25;
export const f = (n: number): number => n * FRAME;

/** 100 px per second: a frame is 4 px. The header column is 100 px, the ruler 28 px. */
export const PX_PER_SEC = 100;
export const PX_PER_FRAME = PX_PER_SEC / 25;
export const HEADER = 100;
export const WIDTH = 1000;
export const HEIGHT = 380;

type ItemInit = Pick<TimelineItem, "id" | "trackId"> & {
  readonly from: number;
  readonly to: number;
  readonly group?: string;
  readonly media?: number | null;
  readonly sourceIn?: number;
  readonly still?: boolean;
  readonly keyframes?: readonly number[];
};

function item(o: ItemInit): TimelineItem {
  const track = TRACKS.find((t) => t.id === o.trackId);
  return {
    id: o.id,
    trackId: o.trackId,
    kind: track?.kind ?? "media",
    start: f(o.from),
    duration: f(o.to - o.from),
    label: o.id,
    ...(o.group !== undefined && { linkedGroup: o.group }),
    ...(o.keyframes !== undefined && { keyframes: o.keyframes.map(f) }),
    source: {
      sourceIn: f(o.sourceIn ?? 0),
      mediaDuration: o.media === undefined ? f(300) : o.media === null ? null : f(o.media),
      ...(o.still === true && { still: true }),
    },
  };
}

const TRACKS: readonly TimelineTrack[] = [
  { id: "L1", name: "L1", kind: "media", locked: true },
  { id: "V3", name: "V3", kind: "media" },
  { id: "V2", name: "V2", kind: "media" },
  { id: "V1", name: "V1", kind: "media", main: true },
  { id: "A1", name: "A1", kind: "audio" },
  { id: "A2", name: "A2", kind: "audio" },
];

/**
 * The standard sequence (frames):
 *
 *     L1#  [K 0-50]                                   locked
 *     V3                                              empty
 *     V2   [X 0-20]    [Y 40-80]    [Z 100-120]       X is a still; Y has keys at 50, 70
 *     V1*  [A 0-30][B 30-60][C 60-90]                 the main track
 *     A1   [a 0-30][b 30-60][c 60-90]                 linked audio of A, B, C
 *     A2   [M 0-70]                                   music
 *
 * A sequence marker sits at 130.
 */
export const MODEL: TimelineModel = {
  fps: FPS,
  tracks: TRACKS,
  items: [
    item({ id: "K", trackId: "L1", from: 0, to: 50 }),
    item({ id: "X", trackId: "V2", from: 0, to: 20, media: null, still: true }),
    item({ id: "Y", trackId: "V2", from: 40, to: 80, media: 200, keyframes: [50, 70] }),
    item({ id: "Z", trackId: "V2", from: 100, to: 120, media: 200, sourceIn: 10 }),
    item({ id: "A", trackId: "V1", from: 0, to: 30, group: "g1" }),
    item({ id: "B", trackId: "V1", from: 30, to: 60, group: "g2", sourceIn: 30 }),
    item({ id: "C", trackId: "V1", from: 60, to: 90, group: "g3", sourceIn: 60 }),
    item({ id: "a", trackId: "A1", from: 0, to: 30, group: "g1" }),
    item({ id: "b", trackId: "A1", from: 30, to: 60, group: "g2", sourceIn: 30 }),
    item({ id: "c", trackId: "A1", from: 60, to: 90, group: "g3", sourceIn: 60 }),
    item({ id: "M", trackId: "A2", from: 0, to: 70, media: 1000 }),
  ],
  markers: [{ id: "m1", t: f(130), label: "beat drop", kind: "user" }],
};

/** The model without some items (another actor deleted them). */
export function without(model: TimelineModel, ...ids: string[]): TimelineModel {
  return { ...model, items: model.items.filter((it) => !ids.includes(it.id)) };
}

export const VIDEO: DroppedMedia = { id: "mv", kind: "video", duration: f(20) };
export const AUDIO: DroppedMedia = { id: "ma", kind: "audio", duration: f(40) };
export const IMAGE: DroppedMedia = { id: "mi", kind: "image", duration: null };

/** A keyframe handler like WP-2C.05's: moves the key of property `opacity` on the item. */
export const KEYFRAMES: KeyframeDragHandler = {
  command: (item, keyframeTime, time, prop) => ({
    name: "keyframe.move",
    args: {
      item: item as never,
      keys: [{ prop: (prop ?? "opacity") as never, t: keyframeTime }],
      to_at: time,
    },
  }),
};

export interface WorldOptions {
  readonly model?: TimelineModel;
  readonly env?: Partial<MachineEnv>;
  /** Supply the keyframe drag handler (WP-2C.05). */
  readonly keyframes?: boolean;
  /** Open keyframe lanes (WP-2C.05). */
  readonly lanes?: readonly ItemLanes[];
}

/** One shared snap index for every world in a test file (`initMath()` must have resolved). */
let sharedSnap: WasmSnapPort | null = null;
function snapPort(): WasmSnapPort {
  sharedSnap ??= createSnapPort();
  return sharedSnap;
}

export class FakeEngine {
  /** Open gestures and their updates, in order. */
  readonly open = new Map<number, MachineCommand[]>();
  readonly closed = new Set<number>();
  /** Committed undo steps: a gesture's last update, or a one-shot command. */
  readonly history: MachineCommand[] = [];
  begins = 0;
  closes = 0;

  apply(intent: MachineIntent): void {
    switch (intent.type) {
      case "begin_tx":
        if (this.open.has(intent.gesture) || this.closed.has(intent.gesture)) {
          throw new Error(`gesture ${intent.gesture} opened twice`);
        }
        if (this.open.size > 0) throw new Error("a second gesture opened while one is open");
        this.open.set(intent.gesture, []);
        this.begins++;
        return;
      case "update_tx": {
        const updates = this.open.get(intent.gesture);
        if (updates === undefined)
          throw new Error(`update for gesture ${intent.gesture}, not open`);
        updates.push(intent.command);
        return;
      }
      case "commit_tx":
      case "cancel_tx": {
        const updates = this.open.get(intent.gesture);
        if (updates === undefined) {
          throw new Error(`${intent.type} for gesture ${intent.gesture}, not open`);
        }
        this.open.delete(intent.gesture);
        this.closed.add(intent.gesture);
        this.closes++;
        const last = updates.at(-1);
        if (intent.type === "commit_tx" && last !== undefined) this.history.push(last);
        return;
      }
      case "command":
        this.history.push(intent.command);
        return;
      default:
        return;
    }
  }

  /** The gesture open now, if any. */
  get current(): number | null {
    const [first] = this.open.keys();
    return first ?? null;
  }
}

export class World {
  model: TimelineModel;
  readonly vp: Viewport;
  layout: TimelineLayout;
  playhead: number;
  state: MachineState;
  readonly engine = new FakeEngine();
  readonly intents: MachineIntent[] = [];
  readonly edges: ChartEdge[] = [];
  readonly deps: MachineDeps;
  private readonly theme = testTheme();
  private readonly lanes: readonly ItemLanes[];

  constructor(o: WorldOptions = {}) {
    this.lanes = o.lanes ?? [];
    this.model = o.model ?? MODEL;
    const env: Partial<MachineEnv> = {
      model: this.model,
      sequence: "S1",
      pxPerSec: PX_PER_SEC,
      playhead: f(150),
      ...o.env,
    };
    this.playhead = env.playhead ?? 0;
    this.vp = {
      pxPerSec: env.pxPerSec ?? PX_PER_SEC,
      scrollX: env.scrollX ?? 0,
      scrollY: env.scrollY ?? 0,
      width: WIDTH,
      height: HEIGHT,
      headerWidth: HEADER,
      rulerHeight: this.theme.controlMd,
    };
    this.layout = this.buildLayout();
    this.state = initialMachineState(env);
    this.deps = {
      hitTest: (x, y) =>
        hitTest(
          {
            layout: this.layout,
            vp: this.vp,
            playhead: this.playhead,
            theme: this.theme,
            flagHalfWidth: 12,
          },
          x,
          y,
        ),
      itemsInRect: (rect) => this.itemsInRect(rect),
      lanes: () => ({
        left: this.vp.headerWidth,
        right: this.vp.width,
        top: this.vp.rulerHeight,
        bottom: this.vp.height,
      }),
      snap: snapPort(),
      ...(o.keyframes === true && { keyframes: KEYFRAMES }),
    };
  }

  private buildLayout(): TimelineLayout {
    const th = this.theme;
    return new TimelineLayout(
      this.model,
      {
        sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
        pad: th.space1,
        laneRow: laneRowHeight(th),
      },
      this.lanes,
    );
  }

  /** Runs one event through the machine, then plays host and engine for its intents. */
  send(event: MachineEvent): MachineTransition {
    const out = transition(this.state, event, this.deps);
    this.state = out.state;
    this.edges.push(...out.edges);
    for (const intent of out.intents) {
      this.intents.push(intent);
      this.engine.apply(intent);
      this.host(intent);
    }
    return out;
  }

  /** The document changed (an agent's edit, a patch): the host tells the machine. */
  setModel(model: TimelineModel): void {
    this.model = model;
    this.layout = this.buildLayout();
    this.send({ type: "env", env: { model } });
  }

  private host(intent: MachineIntent): void {
    const k = FLICKS_PER_SECOND / this.vp.pxPerSec;
    switch (intent.type) {
      case "scroll_by":
        this.vp.scrollX = Math.max(0, this.vp.scrollX + Math.round(intent.dx * k));
        this.vp.scrollY = Math.max(0, this.vp.scrollY + intent.dy);
        return;
      case "scroll_to":
        this.vp.scrollX = intent.scrollX;
        this.vp.scrollY = intent.scrollY;
        return;
      case "seek":
        this.playhead = intent.t;
        return;
      case "zoom_by": {
        const anchorT = xToTime(this.vp, intent.anchorX);
        this.vp.pxPerSec *= intent.factor;
        this.vp.scrollX = Math.max(
          0,
          Math.round(
            anchorT -
              (intent.anchorX - this.vp.headerWidth) * (FLICKS_PER_SECOND / this.vp.pxPerSec),
          ),
        );
        this.syncView();
        return;
      }
      case "zoom_to_range":
        this.vp.pxPerSec =
          ((this.vp.width - this.vp.headerWidth) * FLICKS_PER_SECOND) / (intent.t1 - intent.t0);
        this.vp.scrollX = intent.t0;
        this.syncView();
        return;
      default:
        return;
    }
  }

  /** A zoom changed the view: the host tells the machine the new zoom and scroll. */
  private syncView(): void {
    const out = transition(
      this.state,
      { type: "env", env: { pxPerSec: this.vp.pxPerSec, scrollX: this.vp.scrollX } },
      this.deps,
    );
    this.state = out.state;
  }

  private itemsInRect(rect: Rect): string[] {
    const out: string[] = [];
    const t0 = xToTime(this.vp, rect.x);
    const t1 = xToTime(this.vp, rect.x + rect.w);
    const buf: number[] = [];
    for (let ti = 0; ti < this.layout.trackCount; ti++) {
      const top =
        this.vp.rulerHeight + (this.layout.trackTop[ti] ?? 0) - this.vp.scrollY + this.layout.pad;
      if (top + this.layout.clipHeight(ti) <= rect.y || top >= rect.y + rect.h) continue;
      for (const i of this.layout.query(ti, t0, t1, buf)) {
        const it = this.layout.items[i];
        if (it) out.push(it.id);
      }
    }
    return out;
  }

  // ---- positions ----------------------------------------------------------------------------

  /** Canvas x of timeline time `t` (flicks). */
  xOf(t: number): number {
    return this.vp.headerWidth + ((t - this.vp.scrollX) * this.vp.pxPerSec) / FLICKS_PER_SECOND;
  }

  /** Canvas y of the middle of a track's clips. */
  yOf(trackId: string): number {
    const ti = this.layout.trackIndexById.get(trackId);
    if (ti === undefined) throw new Error(`no track ${trackId}`);
    const top = this.vp.rulerHeight + (this.layout.trackTop[ti] ?? 0) - this.vp.scrollY;
    return top + (this.layout.trackHeight[ti] ?? 0) / 2;
  }

  /** Canvas y of a track's keyframe row. */
  keyframeY(trackId: string): number {
    const ti = this.layout.trackIndexById.get(trackId) ?? 0;
    const clipTop =
      this.vp.rulerHeight + (this.layout.trackTop[ti] ?? 0) - this.vp.scrollY + this.layout.pad;
    return clipTop + this.layout.clipHeight(ti) - this.theme.space1_5 - this.theme.space1;
  }

  item(id: string): TimelineItem {
    const it = this.model.items.find((i) => i.id === id);
    if (it === undefined) throw new Error(`no item ${id}`);
    return it;
  }
}
