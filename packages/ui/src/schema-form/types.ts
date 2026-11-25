// SPDX-License-Identifier: GPL-3.0-or-later
import type { ReactNode } from "react";
import type { KeyframeState } from "../components/KeyframeToggle.tsx";

/** A JSON Schema document or sub-schema, loosely typed: the renderer reads only what it needs. */
export interface JsonSchema {
  type?: string | readonly string[];
  properties?: Readonly<Record<string, JsonSchema>>;
  required?: readonly string[];
  items?: JsonSchema | readonly JsonSchema[];
  prefixItems?: readonly JsonSchema[];
  minItems?: number;
  maxItems?: number;
  enum?: readonly unknown[];
  const?: unknown;
  oneOf?: readonly JsonSchema[];
  anyOf?: readonly JsonSchema[];
  allOf?: readonly JsonSchema[];
  $ref?: string;
  $defs?: Readonly<Record<string, JsonSchema>>;
  definitions?: Readonly<Record<string, JsonSchema>>;
  default?: unknown;
  title?: string;
  description?: string;
  minimum?: number;
  maximum?: number;
  multipleOf?: number;
  format?: string;
  pattern?: string;
  "x-ui"?: UiHints;
  [key: string]: unknown;
}

/** The widgets `x-ui.widget` can name (SCHEMA_HINTS.md). */
export type WidgetName =
  | "number"
  | "slider"
  | "angle"
  | "color"
  | "toggle"
  | "segmented"
  | "select"
  | "vec2"
  | "font"
  | "asset"
  | "curve"
  | "text"
  | "textarea"
  | "hidden";

/** What `#[ui(...)]` writes into a property's `x-ui` (SCHEMA_HINTS.md). */
export interface UiHints {
  widget?: WidgetName;
  label?: string;
  min?: number;
  max?: number;
  step?: number;
  unit?: string;
  /** Decimals shown. */
  precision?: number;
  /** Display = value x scale (opacity 0..1 shown as 0..100 %). */
  scale?: number;
  /** Section the field belongs to (root level and inside objects). */
  group?: string;
  /** Sort key within its container; ties keep the schema order. */
  order?: number;
  placeholder?: string;
  /** `blend == 'custom'`, `!locked`, `mode != 'crf' && advanced`. */
  visibleWhen?: string;
  /** Shows the keyframe diamond when the form is given `keyframes`. */
  keyframeable?: boolean;
  /** Goes into the collapsed "Advanced" section of its container. */
  advanced?: boolean;
  /** `textarea` height in lines. */
  lines?: number;
  /** vec2: starts with the axes linked (proportional edits). */
  linked?: boolean;
  /** asset: the media kind to offer (`video`, `audio`, `image`). */
  kind?: string;
}

/** `start` opens a gesture transaction, `update` streams (at most 60 per second), `end` closes. */
export type Gesture = "start" | "update" | "end";

/** Absent `gesture` = a one-shot change (a toggle, a typed value, a reset): one undo step. */
export interface ChangeMeta {
  gesture?: Gesture | undefined;
}

/**
 * `path` is the dot path of the property inside the params object (`transform.position`,
 * `stops.2.color`; numbers index arrays). It is a params path, never an entity address.
 */
export type OnChange = (path: string, value: unknown, meta: ChangeMeta) => void;

/** The diamond for one field. Same shape as the Slider's `keyframe`. */
export interface FieldKeyframe {
  state: KeyframeState;
  onToggle: () => void;
  onPrev?: (() => void) | undefined;
  onNext?: (() => void) | undefined;
  disabled?: boolean | undefined;
}

export interface AssetOption {
  id: string;
  label: string;
  kind?: string | undefined;
}

/** Props handed to a custom renderer for the `curve` widget (the CurveEditor slots in here). */
export interface CurveRendererProps {
  label: string;
  value: unknown;
  options: readonly FormOption[];
  mixed: boolean;
  disabled: boolean;
  onChange: (value: unknown) => void;
  onCommit: (value: unknown) => void;
}

export interface FormRenderers {
  curve?: ((props: CurveRendererProps) => ReactNode) | undefined;
}

export interface FormOption {
  value: string | number;
  label: string;
}

/** Every visible string; override per locale (`strings` prop). */
export interface FormStrings {
  mixed: string;
  none: string;
  reset: (label: string) => string;
  linkAxes: string;
  linkAxesFor: (label: string) => string;
  addItem: (label: string) => string;
  removeItem: (label: string) => string;
  item: (label: string, index: number) => string;
  advanced: string;
  alpha: string;
  hex: string;
  dial: string;
  fontNote: string;
  unsupported: string;
}

export const DEFAULT_STRINGS: FormStrings = {
  mixed: "Mixed",
  none: "None",
  reset: (label) => `Reset ${label}`,
  linkAxes: "Link X and Y",
  linkAxesFor: (label) => `Link X and Y of ${label}`,
  addItem: (label) => `Add ${label}`,
  removeItem: (label) => `Remove ${label}`,
  item: (label, index) => `${label} ${index + 1}`,
  advanced: "Advanced",
  alpha: "Alpha",
  hex: "hex",
  dial: "dial",
  fontNote: "Font picker arrives with text styling",
  unsupported: "This field has no editor yet",
};
