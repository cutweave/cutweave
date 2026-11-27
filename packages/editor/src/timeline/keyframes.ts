// SPDX-License-Identifier: GPL-3.0-or-later
// Keyframes on the timeline (WP-2C.05), the editor's side: which clips have their keyframe lanes
// open (view state, `viewStore.keyframeLanes`), which keys are selected (session state, here),
// the lanes the renderer draws (a projection of the document), and the edits. Every edit is one
// engine command (`keyframe.move`, `keyframe.delete`, `keyframe.set`, `keyframe.set_interp`,
// `keyframe.paste`); `keyframe.copy` is a query. The callers pass `call`, so a key, a menu
// choice and a test run the same code.
import type {
  Clip,
  Easing,
  Interp,
  ItemId,
  KeyframeClipboard,
  KeyRef,
  Project,
  PropId,
  SequenceId,
} from "@cutweave/protocol";
import { type ItemLanes, KEY_GLYPHS, type KeyframeLane } from "@cutweave/timeline";
import { createStore } from "zustand/vanilla";
import { activeSequence, PROP_PATHS } from "../menus/surfaces/facts.ts";
import { activeDocument } from "../shell/windows.ts";
import { type Call, clipboardStore } from "./clipboard.ts";

/** The system clipboard's format for copied keys (the engine's `KeyframeClipboard`). */
export const KEY_CLIPBOARD_MIME = "application/x-cutweave-keyframes+json";

/** The animatable properties, in the order lanes list them. */
export const ANIMATABLE_PROPS: readonly string[] = Object.keys(PROP_PATHS);

/** A key as the document stores it. */
export interface StoredKey {
  /** Item time, flicks. */
  readonly t: number;
  readonly interp: Interp;
}

/** The keys of an animated property of the clip, or `null` when it is static. */
export function storedKeys(clip: Clip, prop: string): readonly StoredKey[] | null {
  let value: unknown = clip;
  for (const step of PROP_PATHS[prop] ?? []) {
    value = (value as Record<string, unknown> | null)?.[step];
  }
  return (value as { keyframes?: readonly StoredKey[] } | null)?.keyframes ?? null;
}

/** The animated properties of a clip, in lane order. */
export function animatedProps(clip: Clip): string[] {
  return ANIMATABLE_PROPS.filter((prop) => storedKeys(clip, prop) !== null);
}

/** A key by its property and stored item time (the engine's `KeyRef`). */
export interface KeyAddress {
  readonly prop: string;
  readonly t: number;
}

export const sameKey = (a: KeyAddress, b: KeyAddress): boolean => a.prop === b.prop && a.t === b.t;

const refs = (keys: readonly KeyAddress[]): KeyRef[] =>
  keys.map((k) => ({ prop: k.prop as PropId, t: k.t }));

// ---- the key selection ----------------------------------------------------------------------

export interface KeySelection {
  /** The clip whose keys are selected (keys of one clip at a time), or null. */
  readonly item: string | null;
  readonly keys: readonly KeyAddress[];
  /**
   * While the selected keys are dragged: how far they are from their stored times, flicks, so
   * the lanes keep showing them selected until the drag is committed.
   */
  readonly offset: number;
}

const NO_KEYS: readonly KeyAddress[] = [];
export const EMPTY_KEY_SELECTION: KeySelection = { item: null, keys: NO_KEYS, offset: 0 };

export const keySelectionStore = createStore<KeySelection>(() => EMPTY_KEY_SELECTION);

export function selectKeys(item: string | null, keys: readonly KeyAddress[]): void {
  if (item === null || keys.length === 0) clearKeys();
  else keySelectionStore.setState({ item, keys, offset: 0 });
}

export function clearKeys(): void {
  if (keySelectionStore.getState() !== EMPTY_KEY_SELECTION) {
    keySelectionStore.setState(EMPTY_KEY_SELECTION, true);
  }
}

/** Adds the key to the selection, or takes it out (Shift or Ctrl click). */
export function toggleKey(item: string, key: KeyAddress): void {
  const s = keySelectionStore.getState();
  if (s.item !== item) {
    selectKeys(item, [key]);
    return;
  }
  const has = s.keys.some((k) => sameKey(k, key));
  selectKeys(item, has ? s.keys.filter((k) => !sameKey(k, key)) : [...s.keys, key]);
}

/** The selection without keys the document no longer has (an undo, another actor's edit). */
export function pruneKeys(project: Project | null): void {
  const s = keySelectionStore.getState();
  if (s.item === null || s.offset !== 0) return;
  const clip = findClip(project, s.item);
  const kept =
    clip === null
      ? NO_KEYS
      : s.keys.filter((k) => storedKeys(clip, k.prop)?.some((x) => x.t === k.t) === true);
  if (kept.length !== s.keys.length) selectKeys(s.item, kept);
}

/** The clip with this ID in any sequence of the project. */
export function findClip(project: Project | null, id: string): Clip | null {
  if (project === null) return null;
  for (const sequence of Object.values(project.sequences)) {
    for (const track of Object.values(sequence.tracks)) {
      const clip = track.items[id as ItemId];
      if (clip !== undefined) return clip;
    }
  }
  return null;
}

// ---- what is laid over the canvas follows the lanes -----------------------------------------

/**
 * Counts the times the canvas laid its tracks out again because a keyframe lane opened or
 * closed. The header buttons and the marks over clips read their places from the canvas, so
 * they follow this (as they follow scroll and zoom). It never changes while every lane is closed.
 */
export const lanesLaidStore = createStore<number>(() => 0);

export function lanesRelaid(): void {
  lanesLaidStore.setState((n) => n + 1, true);
}

/** Which clips have lanes open and how many each: what decides the track heights. */
export function lanesShapeOf(lanes: readonly ItemLanes[]): string {
  if (lanes.length === 0) return "";
  return lanes.map((group) => `${group.itemId}:${group.lanes.length}`).join(",");
}

// ---- the lanes toggle ---------------------------------------------------------------------

/** The toolbar's Keyframe lanes button: nothing to open, lanes closed, or lanes open. */
export type LanesToggle = "none" | "closed" | "open";

/**
 * Reads the toggle's state for a document, the selected clips and the open lanes, remembering
 * the last answer: asked again about the same three objects (a playhead move, a scroll, another
 * component's render) it looks at no clip.
 */
export function createLanesToggle(): (
  project: Project | null,
  selected: readonly string[],
  open: readonly string[],
) => LanesToggle {
  let last: { project: Project | null; selected: readonly string[]; open: readonly string[] } = {
    project: null,
    selected: NO_IDS,
    open: NO_IDS,
  };
  let state: LanesToggle = "none";
  return (project, selected, open) => {
    if (project === last.project && selected === last.selected && open === last.open) return state;
    last = { project, selected, open };
    let animated = 0;
    let closed = 0;
    for (const id of selected) {
      const clip = findClip(project, id);
      if (clip === null || !ANIMATABLE_PROPS.some((prop) => storedKeys(clip, prop) !== null)) {
        continue;
      }
      animated += 1;
      if (!open.includes(id)) closed += 1;
    }
    state = animated === 0 ? "none" : closed === 0 ? "open" : "closed";
    return state;
  };
}

const NO_IDS: readonly string[] = [];

// ---- the lanes the renderer draws -----------------------------------------------------------

const glyphOf = (interp: Interp): number => Math.max(0, KEY_GLYPHS.indexOf(interp.kind));

interface LaneArrays {
  readonly prop: string;
  readonly times: Float64Array;
  readonly glyphs: Uint8Array;
}

/** Per clip object: the documents are immutable, so an unchanged clip keeps its arrays. */
const arraysCache = new WeakMap<Clip, readonly LaneArrays[]>();

function arraysOf(clip: Clip): readonly LaneArrays[] {
  let out = arraysCache.get(clip);
  if (out === undefined) {
    const built: LaneArrays[] = [];
    for (const prop of ANIMATABLE_PROPS) {
      const keys = storedKeys(clip, prop);
      if (keys === null) continue;
      const times = new Float64Array(keys.length);
      const glyphs = new Uint8Array(keys.length);
      keys.forEach((k, i) => {
        times[i] = clip.start + k.t;
        glyphs[i] = glyphOf(k.interp);
      });
      built.push({ prop, times, glyphs });
    }
    out = built;
    arraysCache.set(clip, out);
  }
  return out;
}

/** Index of `t` in ascending `times`, or -1. */
function indexOfTime(times: Float64Array, t: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((times[mid] as number) < t) lo = mid + 1;
    else hi = mid;
  }
  return times[lo] === t ? lo : -1;
}

/**
 * The lanes of the clips in `open` that are in the sequence and have an animated property, with
 * the selected keys marked. `label` names a property.
 */
export function laneGroups(
  project: Project | null,
  sequence: SequenceId | null,
  open: readonly string[],
  selection: KeySelection,
  label: (prop: string) => string,
): readonly ItemLanes[] {
  // Nothing open (the usual case, and every update of a clip drag): the document is not read.
  if (open.length === 0) return NO_GROUPS;
  const tracks = sequence === null ? undefined : project?.sequences[sequence]?.tracks;
  if (tracks === undefined) return NO_GROUPS;
  const out: ItemLanes[] = [];
  for (const id of open) {
    let clip: Clip | undefined;
    for (const track of Object.values(tracks)) {
      clip = track.items[id as ItemId];
      if (clip !== undefined) break;
    }
    if (clip === undefined) continue;
    const start = clip.start;
    const lanes = arraysOf(clip).map((a): KeyframeLane => {
      const selected: number[] = [];
      if (selection.item === id) {
        for (const k of selection.keys) {
          if (k.prop !== a.prop) continue;
          const i = indexOfTime(a.times, start + k.t + selection.offset);
          if (i >= 0) selected.push(i);
        }
        selected.sort((p, q) => p - q);
      }
      return {
        prop: a.prop,
        label: label(a.prop),
        times: a.times,
        glyphs: a.glyphs,
        ...(selected.length > 0 && { selected }),
      };
    });
    if (lanes.length > 0) out.push({ itemId: id, lanes });
  }
  return out.length === 0 ? NO_GROUPS : out;
}

/** No lane is open: always this one array, so nothing downstream sees a change. */
export const NO_GROUPS: readonly ItemLanes[] = [];

// ---- edits ----------------------------------------------------------------------------------

/** `keyframe.move` for a drag: one key goes to a sequence time, several move by a distance. */
export function moveArgs(
  item: string,
  keys: readonly KeyAddress[],
  from: number,
  to: number,
): { item: ItemId; keys: KeyRef[]; to_at?: number; delta?: number } {
  return keys.length === 1
    ? { item: item as ItemId, keys: refs(keys), to_at: to }
    : { item: item as ItemId, keys: refs(keys), delta: to - from };
}

/** What `keyframe.move` answered: where each key is now. */
interface Moved {
  readonly moved?: readonly { readonly prop: string; readonly t: number }[];
}

export const movedKeys = (output: unknown): KeyAddress[] =>
  ((output as Moved | null)?.moved ?? []).map((m) => ({ prop: m.prop, t: m.t }));

/** Moves the keys by `delta` flicks (a nudge): one undo step. They stay selected. */
export async function nudgeKeys(
  call: Call,
  item: string,
  keys: readonly KeyAddress[],
  delta: number,
): Promise<void> {
  const out = await call("keyframe.move", { item, keys: refs(keys), delta });
  const moved = movedKeys(out);
  if (moved.length > 0) selectKeys(item, moved);
}

/** Deletes the keys: one undo step. */
export async function deleteKeys(
  call: Call,
  item: string,
  keys: readonly KeyAddress[],
): Promise<void> {
  await call("keyframe.delete", { item, keys: refs(keys) });
  clearKeys();
}

/** Adds a key of `prop` at sequence time `at` with the property's value there, and selects it. */
export async function addKey(call: Call, item: string, prop: string, at: number): Promise<void> {
  const out = (await call("keyframe.set", { item, prop, keys: [{ at }] })) as {
    keys?: readonly { prop: string; t: number }[];
  } | null;
  const key = out?.keys?.[0];
  if (key !== undefined) selectKeys(item, [{ prop: key.prop, t: key.t }]);
}

/** Sets how the segments that start at the keys move: a preset name, or the stored form. */
export async function setEasing(
  call: Call,
  item: string,
  keys: readonly KeyAddress[],
  how: { readonly easing: Easing } | { readonly interp: Interp },
): Promise<void> {
  await call("keyframe.set_interp", { item, keys: refs(keys), ...how });
}

// ---- the clipboard --------------------------------------------------------------------------

export interface KeyClipboardState {
  readonly payload: KeyframeClipboard | null;
}

/** What the last Copy of keys put there (the in-app copy of the system clipboard's payload). */
export const keyClipboardStore = createStore<KeyClipboardState>(() => ({ payload: null }));

export function parseKeyPayload(text: string): KeyframeClipboard | null {
  try {
    const value = JSON.parse(text) as Partial<KeyframeClipboard> | null;
    if (
      value !== null &&
      typeof value === "object" &&
      value.version === 1 &&
      Array.isArray(value.keys) &&
      value.keys.length > 0
    ) {
      return value as KeyframeClipboard;
    }
  } catch {
    // not JSON: someone else's clipboard
  }
  return null;
}

let writing = false;

/** Whether the copy event in flight is our own system-clipboard write. */
export function isWritingKeys(): boolean {
  return writing;
}

function writeSystemClipboard(payload: KeyframeClipboard): void {
  if (typeof globalThis.document === "undefined") return;
  const doc = activeDocument();
  const json = JSON.stringify(payload);
  const fill = (event: ClipboardEvent): void => {
    event.clipboardData?.setData(KEY_CLIPBOARD_MIME, json);
    event.preventDefault();
  };
  writing = true;
  doc.addEventListener("copy", fill, { capture: true, once: true });
  try {
    doc.execCommand("copy");
  } catch {
    // The in-app clipboard still works.
  } finally {
    doc.removeEventListener("copy", fill, { capture: true });
    writing = false;
  }
}

/** Copies the keys: the engine's payload goes to both clipboards. Nothing changes. */
export async function copyKeys(
  call: Call,
  item: string,
  keys: readonly KeyAddress[],
): Promise<KeyframeClipboard> {
  const out = (await call("keyframe.copy", { item, keys: refs(keys) })) as {
    clipboard: KeyframeClipboard;
  };
  keyClipboardStore.setState({ payload: out.clipboard });
  // One clipboard: copied keys replace copied clips, as on the system clipboard.
  clipboardStore.getState().setPayload(null);
  writeSystemClipboard(out.clipboard);
  return out.clipboard;
}

/** Pastes copied keys onto `item`, the earliest at sequence time `at`. They become selected. */
export async function pasteKeys(
  call: Call,
  payload: KeyframeClipboard,
  item: string,
  at: number,
): Promise<void> {
  const out = (await call("keyframe.paste", { item, payload, at })) as {
    keys?: readonly { item: string; prop: string; t: number }[];
  } | null;
  const pasted = (out?.keys ?? []).filter((k) => k.item === item);
  if (pasted.length > 0) {
    selectKeys(
      item,
      pasted.map((k) => ({ prop: k.prop, t: k.t })),
    );
  }
}

// ---- reading the selection for menus and keys -----------------------------------------------

/** Every key of the clip in time order (ties in lane order), with its sequence time. */
export function orderedKeys(clip: Clip): (KeyAddress & { readonly at: number })[] {
  const out: (KeyAddress & { at: number; order: number })[] = [];
  animatedProps(clip).forEach((prop, order) => {
    for (const k of storedKeys(clip, prop) ?? []) {
      out.push({ prop, t: k.t, at: clip.start + k.t, order });
    }
  });
  out.sort((a, b) => a.at - b.at || a.order - b.order);
  return out;
}

/** The keys of every property of the clip at item time `t`. */
export function keysAt(clip: Clip, t: number): KeyAddress[] {
  return animatedProps(clip)
    .filter((prop) => storedKeys(clip, prop)?.some((k) => k.t === t) === true)
    .map((prop) => ({ prop, t }));
}

/**
 * The keys an action on the key at sequence time `at` of `prop` (null: the clip's own key row,
 * every property) acts on: the whole key selection when that key is part of it, else that key.
 */
export function keysForTarget(
  item: string,
  prop: string | null,
  at: number,
): { readonly clip: Clip; readonly keys: readonly KeyAddress[] } | null {
  const found = activeSequence();
  if (found === null) return null;
  let clip: Clip | undefined;
  for (const track of Object.values(found.sequence.tracks)) {
    clip = track.items[item as ItemId];
    if (clip !== undefined) break;
  }
  if (clip === undefined) return null;
  const t = at - clip.start;
  if (prop === null) return { clip, keys: keysAt(clip, t) };
  const selection = keySelectionStore.getState();
  const key = { prop, t };
  if (selection.item === item && selection.keys.some((k) => sameKey(k, key))) {
    return { clip, keys: selection.keys };
  }
  return { clip, keys: [key] };
}
