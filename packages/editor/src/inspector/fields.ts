// SPDX-License-Identifier: GPL-3.0-or-later
// What the inspector shows for a clip and how each edit is sent (WP-1M.07), in one typed table.
// The panel renders it row by row and the editor turns an edit into engine commands from it, so
// the schema-driven inspector of P2 replaces the table's source, not the panel's logic.
//
// Edits are engine commands only (arch §5.1): property fields send `item.set_props`; Start sends
// `item.move` and Duration sends `clip.trim` on the end edge, so every timeline edit also has a
// path without a pointer.

import type {
  Animatable,
  Item,
  ItemId,
  ItemProps,
  SequenceId,
  TrackId,
} from "@cutweave/protocol";
import type { FrameRate } from "@cutweave/timeline";
import type { TKey } from "../i18n/index.ts";
import type { GestureUpdate } from "../timeline/gesture.ts";

export type FieldId = "name" | "start" | "duration" | "gain";

/** The sections of hand-built rows. The Video sections come from the schema (`video.ts`). */
export type SectionId = "clip" | "timing" | "audio";

/** A selected clip with what its edits need to know. */
export interface SelectedClip {
  readonly item: Item;
  readonly trackId: TrackId;
  readonly sequence: SequenceId;
  /** The frame rate of the clip's sequence: timecodes resolve against it. */
  readonly fps: FrameRate;
  /** The clip's track is locked: the engine refuses edits (the panel disables the fields). */
  readonly locked: boolean;
}

interface FieldBase {
  readonly id: FieldId;
  readonly section: SectionId;
  readonly label: TKey;
  /** The undo entry's name for a gesture or a typed edit of this field. */
  readonly history: TKey;
}

export interface NumberField extends FieldBase {
  /** `slider`: bounded and dragged; `scrub`: unbounded-ish number field. */
  readonly control: "slider" | "scrub";
  readonly min: number;
  readonly max: number;
  readonly step: number;
  readonly unit?: string;
  readonly precision?: number;
  read(item: Item): number;
  updates(clip: SelectedClip, value: number): GestureUpdate[];
}

export interface TextField extends FieldBase {
  readonly control: "text";
  read(item: Item): string;
  updates(clip: SelectedClip, value: string): GestureUpdate[];
}

export interface TimeField extends FieldBase {
  readonly control: "time";
  /** Flicks. */
  read(item: Item): number;
  updates(clip: SelectedClip, flicks: number): GestureUpdate[];
}

export type FieldDef = NumberField | TextField | TimeField;

/** An `item.set_props` with only the given properties set (`null` = keep). */
export function propsUpdate(item: ItemId, patch: Partial<ItemProps>): GestureUpdate {
  return {
    name: "item.set_props",
    input: {
      item,
      props: {
        name: null,
        enabled: null,
        opacity: null,
        audio_gain: null,
        audio_pan: null,
        transform: null,
        crop: null,
        flip: null,
        blend_mode: null,
        ...patch,
      },
    },
  };
}

/**
 * The value of a property that may be animated: the static value, or the first key's (what the
 * engine's `Animatable::value` returns). The Video sections show the value at the playhead
 * instead (`video.ts`); this is for the rows that are not animated yet (Gain, WP-2H.03b).
 */
export function staticValue<T>(value: Animatable<T>): T {
  if (typeof value === "object" && value !== null && "keyframes" in value) {
    return (value.keyframes[0] as { value: T }).value;
  }
  return value as T;
}

/** The rows of the clip inspector, top to bottom. */
export const FIELDS: readonly FieldDef[] = [
  {
    id: "name",
    section: "clip",
    control: "text",
    label: "inspector.fields.name",
    history: "inspector.history.name",
    read: (item) => item.name ?? "",
    updates: ({ item }, value) => [propsUpdate(item.id, { name: value })],
  },
  {
    id: "start",
    section: "timing",
    control: "time",
    label: "inspector.fields.start",
    history: "inspector.history.start",
    read: (item) => item.start,
    updates: ({ item }, flicks) => [{ name: "item.move", input: { item: item.id, start: flicks } }],
  },
  {
    id: "duration",
    section: "timing",
    control: "time",
    label: "inspector.fields.duration",
    history: "inspector.history.duration",
    read: (item) => item.duration,
    // The end edge moves; the start stays (a trim, so linked partners follow).
    updates: ({ item }, flicks) => [
      { name: "clip.trim", input: { item: item.id, edge: "end", to: item.start + flicks } },
    ],
  },
  {
    id: "gain",
    section: "audio",
    control: "scrub",
    label: "inspector.fields.gain",
    history: "inspector.history.gain",
    min: -144,
    max: 48,
    step: 0.5,
    precision: 1,
    unit: "dB",
    read: (item) => staticValue(item.audio.gain),
    updates: ({ item }, value) => [propsUpdate(item.id, { audio_gain: value })],
  },
];

export const SECTIONS: readonly { readonly id: SectionId; readonly label: TKey }[] = [
  { id: "clip", label: "inspector.clip" },
  { id: "timing", label: "inspector.timing" },
  { id: "audio", label: "inspector.audio" },
];

/** One field's value across the selection: the shared value, or `mixed`. */
export type FieldValue<V> =
  | { readonly mixed: false; readonly value: V }
  | { readonly mixed: true; readonly value: V };

/** Reads a field over every selected clip; `value` is the first clip's when they differ. */
export function readField<V>(
  clips: readonly SelectedClip[],
  read: (item: Item) => V,
): FieldValue<V> {
  const first = clips[0];
  if (first === undefined) throw new Error("readField needs a selected clip");
  const value = read(first.item);
  const mixed = clips.some((c) => read(c.item) !== value);
  return { mixed, value };
}
