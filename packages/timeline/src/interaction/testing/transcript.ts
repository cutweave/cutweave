// SPDX-License-Identifier: GPL-3.0-or-later
// Test support (not built): transcripts. A transcript is a recorded event stream (pointer, keys,
// frames, engine answers) replayed against a `World`; it asserts the intents the machine emitted,
// written compactly, the undo steps the fake engine committed (the final document, as commands)
// and the state the machine ends in.
import type { SnapGuide } from "../../model.ts";
import type { StateName } from "../state.ts";
import type {
  DroppedMedia,
  IntentType,
  MachineAction,
  MachineCommand,
  MachineEnv,
  MachineEvent,
  MachineIntent,
  Modifiers,
} from "../types.ts";
import { FRAME, PX_PER_FRAME, type World, type WorldOptions } from "./world.ts";

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** A canvas position, resolved against the world when the step runs (scrolling moves things). */
export type Pos = (w: World) => Point;

export type Step = (w: World) => void;

export interface Transcript {
  readonly name: string;
  readonly world?: WorldOptions;
  readonly steps: readonly Step[];
  /** The intents, formatted (`fmt`), of the types in `show` (default `DEFAULT_SHOWN`). */
  readonly expect: readonly string[];
  /** The committed undo steps, formatted. Default: not checked. */
  readonly history?: readonly string[];
  readonly end: StateName;
  readonly show?: readonly IntentType[];
}

/** The intent types a transcript shows unless it asks for others. */
export const DEFAULT_SHOWN: readonly IntentType[] = [
  "begin_tx",
  "update_tx",
  "commit_tx",
  "cancel_tx",
  "command",
  "set_selection",
  "notice",
  "seek",
  "set_tool",
  "set_snapping",
  "zoom_by",
  "zoom_to_range",
  "scroll_to",
];

// ---- positions ------------------------------------------------------------------------------

/** A clip's body (its middle), start edge, end edge, or its `n`-th keyframe. */
export function clip(id: string, part: "body" | "start" | "end" | number = "body"): Pos {
  return (w) => {
    const it = w.item(id);
    const x0 = w.xOf(it.start);
    const x1 = w.xOf(it.start + it.duration);
    if (typeof part === "number") {
      const t = it.keyframes?.[part];
      if (t === undefined) throw new Error(`${id} has no keyframe ${part}`);
      return { x: w.xOf(t), y: w.keyframeY(it.trackId) };
    }
    const y = w.yOf(it.trackId);
    if (part === "start") return { x: x0 + 2, y };
    if (part === "end") return { x: x1 - 2, y };
    return { x: (x0 + x1) / 2, y };
  };
}

/** A point on a track's lane at frame `frame`. */
export const lane =
  (track: string, frame: number): Pos =>
  (w) => ({ x: w.xOf(frame * FRAME), y: w.yOf(track) });

/** The ruler at frame `frame`. */
export const ruler =
  (frame: number): Pos =>
  (w) => ({ x: w.xOf(frame * FRAME), y: 10 });

/** A track's header. */
export const header =
  (track: string): Pos =>
  (w) => ({ x: 50, y: w.yOf(track) });

/** An absolute canvas point. */
export const point =
  (x: number, y: number): Pos =>
  () => ({ x, y });

/** `pos` moved by `frames` (4 px each) and `dy` px. */
export const by =
  (pos: Pos, frames: number, dy = 0): Pos =>
  (w) => {
    const p = pos(w);
    return { x: p.x + frames * PX_PER_FRAME, y: p.y + dy };
  };

/** `pos` moved by `dx`, `dy` CSS px. */
export const px =
  (pos: Pos, dx: number, dy = 0): Pos =>
  (w) => {
    const p = pos(w);
    return { x: p.x + dx, y: p.y + dy };
  };

/** `pos` moved vertically onto a track (same x). */
export const onto =
  (pos: Pos, track: string): Pos =>
  (w) => ({ x: pos(w).x, y: w.yOf(track) });

// ---- steps ----------------------------------------------------------------------------------

export const send =
  (event: MachineEvent): Step =>
  (w) => {
    w.send(event);
  };

export const move =
  (pos: Pos, mods: Modifiers = {}): Step =>
  (w) => {
    w.send({ type: "pointer.move", ...pos(w), ...mods });
  };

export const down =
  (pos: Pos, mods: Modifiers & { button?: number } = {}): Step =>
  (w) => {
    w.send({ type: "pointer.down", ...pos(w), ...mods });
  };

export const up =
  (pos: Pos, mods: Modifiers = {}): Step =>
  (w) => {
    w.send({ type: "pointer.up", ...pos(w), ...mods });
  };

export const frame: Step = (w) => {
  w.send({ type: "frame" });
};

export const frames =
  (n: number): Step =>
  (w) => {
    for (let i = 0; i < n; i++) w.send({ type: "frame" });
  };

export const key =
  (k: string, mods: Modifiers = {}): Step =>
  (w) => {
    w.send({ type: "key.down", key: k, ...mods });
  };

export const keyUp =
  (k: string, mods: Modifiers = {}): Step =>
  (w) => {
    w.send({ type: "key.up", key: k, ...mods });
  };

export const action =
  (a: MachineAction): Step =>
  (w) => {
    w.send({ type: "action", action: a });
  };

export const env =
  (patch: Partial<MachineEnv>): Step =>
  (w) => {
    w.send({ type: "env", env: patch });
  };

export const blur: Step = (w) => {
  w.send({ type: "blur" });
};

export const pointerCancel: Step = (w) => {
  w.send({ type: "pointer.cancel" });
};

export const leave: Step = (w) => {
  w.send({ type: "pointer.leave" });
};

export const mediaOver =
  (pos: Pos, media: DroppedMedia, mods: Modifiers = {}): Step =>
  (w) => {
    w.send({ type: "media.over", media, ...pos(w), ...mods });
  };

export const mediaDrop =
  (pos: Pos, media: DroppedMedia): Step =>
  (w) => {
    w.send({ type: "media.drop", media, ...pos(w) });
  };

export const mediaLeave: Step = (w) => {
  w.send({ type: "media.leave" });
};

/** The engine answers the open gesture's latest update. */
export const accepted =
  (clamped = false): Step =>
  (w) => {
    w.send({ type: "engine.result", gesture: current(w), ok: true, clamped });
  };

export const refused =
  (code: string, reason?: string, holder?: string): Step =>
  (w) => {
    w.send({
      type: "engine.result",
      gesture: current(w),
      ok: false,
      code,
      ...(reason !== undefined && { reason }),
      ...(holder !== undefined && { holder }),
    });
  };

/** The engine cancelled the open gesture itself (`gesture.cancelled{reason:"idle_timeout"}`). */
export const engineCancelled =
  (reason = "idle_timeout"): Step =>
  (w) => {
    w.send({ type: "engine.cancelled", gesture: current(w), reason });
  };

function current(w: World): number {
  const g = w.engine.current;
  if (g === null) throw new Error("no gesture is open");
  return g;
}

/** Press, travel past the threshold, then each of `via` and `to` (a frame after each), release. */
export function drag(
  from: Pos,
  to: Pos,
  o: { mods?: Modifiers; via?: readonly Pos[]; release?: boolean } = {},
): Step[] {
  const mods = o.mods ?? {};
  const steps: Step[] = [down(from, mods), move(px(from, 4), mods)];
  for (const p of [...(o.via ?? []), to]) steps.push(move(p, mods), frame);
  if (o.release !== false) steps.push(up(to, mods));
  return steps;
}

// ---- formatting -----------------------------------------------------------------------------

/** Times in whole frames (`45f`), otherwise flicks. */
export function fmtTime(t: number): string {
  return t % FRAME === 0 ? `${t / FRAME}f` : String(t);
}

const TIME_KEYS = new Set(["start", "at", "to", "delta", "offset", "to_at", "t"]);

function fmtValue(k: string, v: unknown): string {
  if (Array.isArray(v)) return `[${v.map((x) => fmtValue(k, x)).join(",")}]`;
  if (typeof v === "number" && TIME_KEYS.has(k)) return fmtTime(v);
  if (typeof v === "object" && v !== null) {
    return `{${Object.entries(v)
      .map(([kk, vv]) => `${kk}=${fmtValue(kk, vv)}`)
      .join(" ")}}`;
  }
  return String(v);
}

export function fmtCommand(c: MachineCommand): string {
  const args = Object.entries(c.args as Record<string, unknown>)
    .map(([k, v]) => `${k}=${fmtValue(k, v)}`)
    .join(" ");
  return `${c.name} ${args}`;
}

function fmtGuide(g: SnapGuide): string {
  return typeof g === "number" ? `@${fmtTime(g)}` : `${g.kind}@${fmtTime(g.t)}`;
}

export function fmt(i: MachineIntent): string {
  switch (i.type) {
    case "begin_tx":
      return `begin #${i.gesture} ${i.edit}`;
    case "update_tx":
      return `update #${i.gesture} ${fmtCommand(i.command)}`;
    case "commit_tx":
      return `commit #${i.gesture}`;
    case "cancel_tx":
      return `cancel #${i.gesture} ${i.reason}`;
    case "command":
      return `command ${fmtCommand(i.command)}`;
    case "set_selection":
      return `select [${i.ids.join(",")}]`;
    case "set_cursor":
      return `cursor ${i.cursor}`;
    case "set_tool":
      return `tool ${i.tool}`;
    case "set_snapping":
      return `snapping ${i.on ? "on" : "off"}`;
    case "seek":
      return `seek ${fmtTime(i.t)}`;
    case "scroll_to":
      return `scroll_to x=${fmtTime(i.scrollX)} y=${i.scrollY}`;
    case "scroll_by":
      return `scroll_by ${i.dx},${i.dy}`;
    case "zoom_by":
      return `zoom_by ${i.factor} at x=${i.anchorX}`;
    case "zoom_to_range":
      return `zoom_to ${fmtTime(i.t0)}..${fmtTime(i.t1)}`;
    case "snap_guides":
      return `guides [${i.guides.map(fmtGuide).join(",")}]`;
    case "marquee":
      return i.rect === null
        ? "marquee none"
        : `marquee ${i.rect.x},${i.rect.y} ${i.rect.w}x${i.rect.h}`;
    case "preview":
      return i.preview === null
        ? "preview none"
        : `preview ${i.preview.trackId} ${fmtTime(i.preview.start)}..${fmtTime(i.preview.end)} ${i.preview.mode}`;
    case "notice":
      return `notice ${i.code}`;
  }
}

export interface Replay {
  readonly world: World;
  /** Every intent, formatted. */
  readonly all: readonly string[];
  /** The intents of the shown types, formatted. */
  readonly shown: readonly string[];
  readonly history: readonly string[];
}

export function replay(t: Transcript, make: (o: WorldOptions | undefined) => World): Replay {
  const world = make(t.world);
  for (const step of t.steps) step(world);
  const show = new Set(t.show ?? DEFAULT_SHOWN);
  return {
    world,
    all: world.intents.map(fmt),
    shown: world.intents.filter((i) => show.has(i.type)).map(fmt),
    history: world.engine.history.map(fmtCommand),
  };
}
