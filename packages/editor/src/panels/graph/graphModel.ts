// SPDX-License-Identifier: GPL-3.0-or-later
// The curves the graph editor draws (WP-2C.07): a projection of the document. Each animated
// property of a selected clip is one curve, or two for a two-dimensional one (X and Y), with its
// keys in sequence time and a sampler that evaluates the stored animatable with the engine's own
// math (the shared wasm module), so the curve drawn is the curve rendered. Documents are
// immutable: an unchanged clip keeps its arrays, an unchanged animatable its compiled form.
import type { Clip, Interp, ItemId, Project, SequenceId } from "@cutweave/protocol";
import { anim, type CompiledAnim } from "@cutweave/protocol/wasm";
import type { CurveValue, GraphCurve, GraphPointRef } from "@cutweave/ui";
import { PROP_PATHS } from "../../menus/surfaces/facts.ts";
import { ANIMATABLE_PROPS, type KeyAddress, type KeySelection } from "../../timeline/keyframes.ts";

/** A key as the document stores it. */
export interface StoredGraphKey {
  readonly t: number;
  readonly value: number | boolean | { readonly x: number; readonly y: number };
  readonly interp: Interp;
}

export type ValueShape = "scalar" | "vec2" | "bool";

/** A curve of the graph with where it comes from. */
export interface EditorCurve extends GraphCurve {
  readonly item: string;
  readonly prop: string;
  /** 0, or 1 for the Y of a two-dimensional property. */
  readonly comp: number;
  readonly shape: ValueShape;
  /** The keys' stored item times (flicks); `times` are these plus the clip's start. */
  readonly itemTimes: Float64Array;
  /** The clip's start on the sequence (flicks). */
  readonly start: number;
  readonly duration: number;
  /** The stored keys, by index. */
  readonly keys: readonly StoredGraphKey[];
}

/**
 * The stored range of each property (keyframes §7, `cutweave-model` `animate/props.rs`), so a
 * drag stops at the edge instead of being refused there. The engine remains the authority.
 */
const RANGES: Readonly<Record<string, readonly [number, number]>> = {
  "transform.scale": [0, Number.POSITIVE_INFINITY],
  opacity: [0, 1],
  "crop.left": [0, 1],
  "crop.top": [0, 1],
  "crop.right": [0, 1],
  "crop.bottom": [0, 1],
  audio_gain: [-144, 48],
  audio_pan: [-1, 1],
};

/** `v` kept inside the property's stored range. */
export function clampToProp(prop: string, v: number): number {
  const range = RANGES[prop];
  return range === undefined ? v : Math.min(range[1], Math.max(range[0], v));
}

/** What one arrow key changes a property's value by. */
export function valueStep(prop: string): number {
  if (prop === "transform.position" || prop === "transform.anchor") return 1;
  if (prop === "transform.rotation") return 1;
  if (prop === "audio_gain") return 0.1;
  return 0.01;
}

export const curveId = (item: string, prop: string, comp: number): string =>
  `${item}|${prop}|${comp}`;

/** The stored animatable of a property of the clip (a value, or `{keyframes}`). */
function animatableOf(clip: Clip, prop: string): unknown {
  let value: unknown = clip;
  for (const step of PROP_PATHS[prop] ?? []) {
    value = (value as Record<string, unknown> | null)?.[step];
  }
  return value;
}

const keysOf = (animatable: unknown): readonly StoredGraphKey[] | null =>
  (animatable as { keyframes?: readonly StoredGraphKey[] } | null)?.keyframes ?? null;

const shapeOf = (value: StoredGraphKey["value"]): ValueShape =>
  typeof value === "boolean" ? "bool" : typeof value === "number" ? "scalar" : "vec2";

const component = (value: StoredGraphKey["value"], comp: number): number =>
  typeof value === "boolean"
    ? Number(value)
    : typeof value === "number"
      ? value
      : comp === 0
        ? value.x
        : value.y;

/** The compiled animatables of the curves in use; the ones no longer drawn are freed. */
export class CompiledCache {
  #live = new Map<
    object,
    { compiled: CompiledAnim; used: boolean; buffer: Float64Array; key: string }
  >();

  get size(): number {
    return this.#live.size;
  }

  /** Marks every entry unused; `sweep()` then frees the ones no curve asked for. */
  begin(): void {
    for (const entry of this.#live.values()) entry.used = false;
  }

  /** Values of the animatable at `n` times from `a` to `b` (item time), all components. */
  range(animatable: object, prop: string, a: number, b: number, n: number): Float64Array {
    let entry = this.#live.get(animatable);
    if (entry === undefined) {
      entry = {
        compiled: anim.compile(animatable as never, { prop: prop as never }),
        used: true,
        buffer: new Float64Array(0),
        key: "",
      };
      this.#live.set(animatable, entry);
    }
    // The X and the Y curve of a position ask for the same samples: evaluated once.
    const key = `${a}:${b}:${n}`;
    if (entry.key !== key) {
      const size = n * entry.compiled.components;
      if (entry.buffer.length !== size) entry.buffer = new Float64Array(size);
      entry.compiled.evalRange(a, b, n, entry.buffer);
      entry.key = key;
    }
    return entry.buffer;
  }

  touch(animatable: object): void {
    const entry = this.#live.get(animatable);
    if (entry !== undefined) entry.used = true;
  }

  components(animatable: object): number {
    return this.#live.get(animatable)?.compiled.components ?? 1;
  }

  sweep(): void {
    for (const [animatable, entry] of this.#live) {
      if (entry.used) continue;
      entry.compiled.free();
      this.#live.delete(animatable);
    }
  }

  dispose(): void {
    for (const entry of this.#live.values()) entry.compiled.free();
    this.#live.clear();
  }
}

export interface CurveInputs {
  readonly project: Project | null;
  readonly sequence: SequenceId | null;
  /** The selected clips, in selection order. */
  readonly items: readonly string[];
  readonly hidden: readonly string[];
  /** A property's name ("Position"). */
  label(prop: string): string;
  /** A clip's name, used when several clips are shown. */
  clipLabel(item: string): string;
}

/** A property of a clip that has curves, shown or hidden (the curve list's rows). */
export interface CurveGroup {
  readonly item: string;
  readonly prop: string;
  readonly label: string;
  readonly colors: readonly number[];
  readonly hidden: boolean;
}

export interface BuiltCurves {
  readonly curves: readonly EditorCurve[];
  readonly groups: readonly CurveGroup[];
  /** The clip objects the curves were built from (the same objects: nothing to rebuild). */
  readonly clips: readonly Clip[];
}

export const NO_CURVES: BuiltCurves = { curves: [], groups: [], clips: [] };

/** The most clips whose curves are shown at once. */
export const MAX_CLIPS = 8;

/** The selected clips that are in the sequence, with whether their track is locked. */
export function selectedClips(
  project: Project | null,
  sequence: SequenceId | null,
  items: readonly string[],
): { clip: Clip; locked: boolean }[] {
  const tracks = sequence === null ? undefined : project?.sequences[sequence]?.tracks;
  if (tracks === undefined) return [];
  const out: { clip: Clip; locked: boolean }[] = [];
  for (const id of items) {
    if (out.length >= MAX_CLIPS) break;
    for (const track of Object.values(tracks)) {
      const clip = track.items[id as ItemId];
      if (clip !== undefined) {
        out.push({ clip, locked: track.locked === true });
        break;
      }
    }
  }
  return out;
}

/** Builds the curves of the selected clips' animated properties. */
export function buildCurves(inputs: CurveInputs, cache: CompiledCache): BuiltCurves {
  const found = selectedClips(inputs.project, inputs.sequence, inputs.items);
  const curves: EditorCurve[] = [];
  const groups: CurveGroup[] = [];
  // A clip's name goes before its properties only when more than one clip has curves.
  const several =
    found.filter(({ clip }) =>
      ANIMATABLE_PROPS.some((prop) => (keysOf(animatableOf(clip, prop))?.length ?? 0) > 0),
    ).length > 1;
  let color = 0;
  cache.begin();
  for (const { clip, locked } of found) {
    for (const prop of ANIMATABLE_PROPS) {
      const animatable = animatableOf(clip, prop);
      const keys = keysOf(animatable);
      if (keys === null || keys.length === 0) continue;
      const shape = shapeOf((keys[0] as StoredGraphKey).value);
      const comps = shape === "vec2" ? 2 : 1;
      const hidden = inputs.hidden.includes(prop);
      const name = several
        ? `${inputs.clipLabel(clip.id)} · ${inputs.label(prop)}`
        : inputs.label(prop);
      const colors: number[] = [];
      for (let comp = 0; comp < comps; comp++) colors.push(color + comp);
      color += comps;
      groups.push({ item: clip.id, prop, label: name, colors, hidden });
      if (hidden) continue;
      const n = keys.length;
      const itemTimes = new Float64Array(n);
      const times = new Float64Array(n);
      const interps = keys.map((k) => k.interp as CurveValue);
      for (let i = 0; i < n; i++) {
        const t = (keys[i] as StoredGraphKey).t;
        itemTimes[i] = t;
        times[i] = clip.start + t;
      }
      const source = animatable as object;
      const start = clip.start;
      for (let comp = 0; comp < comps; comp++) {
        const values = new Float64Array(n);
        for (let i = 0; i < n; i++) values[i] = component((keys[i] as StoredGraphKey).value, comp);
        curves.push({
          id: curveId(clip.id, prop, comp),
          label: comps === 2 ? `${name} ${comp === 0 ? "X" : "Y"}` : name,
          color: colors[comp] as number,
          times,
          values,
          interps,
          locked,
          item: clip.id,
          prop,
          comp,
          shape,
          itemTimes,
          start,
          duration: clip.duration,
          keys,
          sample(t0, t1, count, out) {
            const a = Math.round(t0 - start);
            const b = Math.max(a + 1, Math.round(t1 - start));
            const all = cache.range(source, prop, a, b, count);
            const stride = all.length / count;
            if (stride === 1) out.set(all.subarray(0, count));
            else for (let i = 0; i < count; i++) out[i] = all[i * stride + comp] as number;
          },
        });
      }
      cache.touch(source);
    }
  }
  cache.sweep();
  return { curves, groups, clips: found.map((f) => f.clip) };
}

/** Whether `built` was made from exactly these clip objects (nothing to rebuild). */
export function sameClips(built: BuiltCurves, clips: readonly { clip: Clip }[]): boolean {
  if (built.clips.length !== clips.length) return false;
  for (let i = 0; i < clips.length; i++) {
    if (built.clips[i] !== (clips[i] as { clip: Clip }).clip) return false;
  }
  return true;
}

/** Index of `t` in ascending `times`, or -1. */
export function indexOfTime(times: ArrayLike<number>, t: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((times[mid] as number) < t) lo = mid + 1;
    else hi = mid;
  }
  return times[lo] === t ? lo : -1;
}

/** The graph's points for the selected keys: every component the mask lets through. */
export function pointsOf(
  curves: readonly EditorCurve[],
  selection: KeySelection,
  mask: readonly string[] | null,
): GraphPointRef[] {
  const out: GraphPointRef[] = [];
  if (selection.item === null || selection.keys.length === 0) return out;
  for (const curve of curves) {
    if (curve.item !== selection.item) continue;
    if (mask !== null && !mask.includes(`${curve.prop}|${curve.comp}`)) continue;
    for (const key of selection.keys) {
      if (key.prop !== curve.prop) continue;
      const index = indexOfTime(curve.itemTimes, key.t + selection.offset);
      if (index >= 0) out.push({ curve: curve.id, index });
    }
  }
  return out;
}

/** The key of a point of the graph. */
export function keyOfPoint(
  curves: readonly EditorCurve[],
  point: GraphPointRef,
): { curve: EditorCurve; key: KeyAddress } | null {
  const curve = curves.find((c) => c.id === point.curve);
  const t = curve?.itemTimes[point.index];
  return curve === undefined || t === undefined ? null : { curve, key: { prop: curve.prop, t } };
}
