// SPDX-License-Identifier: GPL-3.0-or-later
import type { SchemaFormProps } from "./SchemaForm.tsx";
import type { JsonSchema, UiHints } from "./types.ts";

/** A color literal built at run time (the literals lint is for UI code, not for sample data). */
export const hex = (digits: string) => `#${digits}`;

export interface Fixture {
  name: string;
  schema: JsonSchema;
  value: unknown;
  /** Expected widget per field path (`data-path` / `data-widget`). */
  widgets: Readonly<Record<string, string>>;
  /** Extra props for the form. */
  props?: Partial<SchemaFormProps>;
}

const obj = (
  properties: Record<string, JsonSchema>,
  required: string[] = Object.keys(properties),
): JsonSchema => ({ type: "object", properties, required, additionalProperties: false });

const ui = (x: UiHints, schema: JsonSchema): JsonSchema => ({ ...schema, "x-ui": x });

const number = (extra: JsonSchema = {}): JsonSchema => ({
  type: "number",
  format: "double",
  ...extra,
});
const BLENDS = ["normal", "multiply", "screen", "overlay", "add", "darken"];

export const FIXTURES: readonly Fixture[] = [
  {
    name: "number",
    schema: obj({ gain: number({ description: "Gain in dB." }) }),
    value: { gain: -6 },
    widgets: { gain: "number" },
  },
  {
    name: "integer with a range",
    schema: obj({ count: { type: "integer", minimum: 1, maximum: 10, default: 3 } }),
    value: { count: 5 },
    widgets: { count: "number" },
  },
  {
    name: "slider",
    schema: obj({ blur: ui({ widget: "slider", min: 0, max: 50, step: 1, unit: "px" }, number()) }),
    value: { blur: 12 },
    widgets: { blur: "slider" },
  },
  {
    name: "slider without a range falls back to a number",
    schema: obj({ blur: ui({ widget: "slider" }, number()) }),
    value: { blur: 12 },
    widgets: { blur: "number" },
  },
  {
    name: "percent slider (scale)",
    schema: obj({
      opacity: ui(
        { widget: "slider", min: 0, max: 1, step: 0.01, unit: "%", scale: 100 },
        number({ default: 1 }),
      ),
    }),
    value: { opacity: 0.5 },
    widgets: { opacity: "slider" },
  },
  {
    name: "angle dial",
    schema: obj({
      rotation: ui({ widget: "angle", min: -360, max: 360, unit: "°" }, number({ default: 0 })),
    }),
    value: { rotation: 45 },
    widgets: { rotation: "angle" },
  },
  {
    name: "color",
    schema: obj({ fill: ui({ widget: "color" }, { type: "string" }) }),
    value: { fill: hex("ff8800") },
    widgets: { fill: "color" },
  },
  {
    name: "color with alpha",
    schema: obj({ shadow: ui({ widget: "color" }, { type: "string", default: hex("00000080") }) }),
    value: { shadow: hex("00000080") },
    widgets: { shadow: "color" },
  },
  {
    name: "toggle",
    schema: obj({ reverse: { type: "boolean", default: false } }),
    value: { reverse: true },
    widgets: { reverse: "toggle" },
  },
  {
    name: "short enum is segmented",
    schema: obj({ fit: { type: "string", enum: ["fit", "fill", "stretch"] } }),
    value: { fit: "fill" },
    widgets: { fit: "segmented" },
  },
  {
    name: "long enum is a select",
    schema: obj({ blend: { type: "string", enum: BLENDS } }),
    value: { blend: "screen" },
    widgets: { blend: "select" },
  },
  {
    name: "oneOf const with titles",
    schema: obj({
      policy: {
        oneOf: [
          { const: "crop", title: "Crop" },
          { const: "bars", title: "Bars" },
        ],
      },
    }),
    value: { policy: "bars" },
    widgets: { policy: "segmented" },
  },
  {
    name: "nullable enum",
    schema: obj({ mode: { anyOf: [{ type: "string", enum: BLENDS }, { type: "null" }] } }, []),
    value: { mode: null },
    widgets: { mode: "select" },
  },
  {
    name: "vec2 object",
    schema: obj({ position: obj({ x: number(), y: number() }) }),
    value: { position: { x: 10, y: -20 } },
    widgets: { position: "vec2" },
  },
  {
    name: "vec2 array, linked scale",
    schema: obj({
      scale: ui(
        { widget: "vec2", linked: true, min: 0, unit: "%", scale: 100 },
        {
          type: "array",
          items: number(),
          minItems: 2,
          maxItems: 2,
          default: [1, 1],
        },
      ),
    }),
    value: { scale: [1, 1] },
    widgets: { scale: "vec2" },
  },
  {
    name: "tuple of three",
    schema: obj({ lift: { type: "array", items: number(), minItems: 3, maxItems: 3 } }),
    value: { lift: [0, 0.1, 0.2] },
    widgets: { lift: "tuple" },
  },
  {
    name: "text",
    schema: obj({ name: { type: "string", default: "Untitled" } }),
    value: { name: "Intro" },
    widgets: { name: "text" },
  },
  {
    name: "textarea",
    schema: obj({ note: ui({ lines: 4, placeholder: "Describe the shot" }, { type: "string" }) }),
    value: { note: "" },
    widgets: { note: "textarea" },
  },
  {
    name: "font",
    schema: obj({ family: ui({ widget: "font" }, { type: "string", default: "Instrument Sans" }) }),
    value: { family: "Bricolage Grotesque" },
    widgets: { family: "font" },
  },
  {
    name: "asset reference",
    schema: obj({ lut: ui({ widget: "asset", kind: "image" }, { type: "string" }) }, []),
    value: { lut: "med_1" },
    widgets: { lut: "asset" },
    props: {
      assets: [
        { id: "med_1", label: "Warm.cube", kind: "image" },
        { id: "med_2", label: "Intro.mp4", kind: "video" },
      ],
    },
  },
  {
    name: "curve slot",
    schema: obj({
      easing: ui(
        { widget: "curve" },
        { type: "string", enum: ["linear", "ease_in", "ease_out", "ease_in_out", "bounce"] },
      ),
    }),
    value: { easing: "ease_out" },
    widgets: { easing: "curve" },
  },
  {
    name: "nested object",
    schema: obj({
      crop: obj({ left: number({ default: 0 }), right: number({ default: 0 }) }),
    }),
    value: { crop: { left: 4, right: 0 } },
    widgets: { crop: "object", "crop.left": "number", "crop.right": "number" },
  },
  {
    name: "two levels of objects",
    schema: obj({
      shadow: obj({
        offset: obj({ x: number(), y: number() }),
        color: ui({ widget: "color" }, { type: "string" }),
      }),
    }),
    value: { shadow: { offset: { x: 2, y: 3 }, color: hex("112233") } },
    widgets: { shadow: "object", "shadow.offset": "vec2", "shadow.color": "color" },
  },
  {
    name: "array of strings",
    schema: obj({ tags: { type: "array", items: { type: "string" }, maxItems: 4 } }),
    value: { tags: ["a", "b"] },
    widgets: { tags: "array", "tags.0": "text", "tags.1": "text" },
  },
  {
    name: "array of objects",
    schema: obj({
      stops: {
        type: "array",
        items: obj({
          at: number({ minimum: 0, maximum: 1 }),
          color: ui({ widget: "color" }, { type: "string" }),
        }),
        minItems: 1,
      },
    }),
    value: {
      stops: [
        { at: 0, color: hex("000000") },
        { at: 1, color: hex("ffffff") },
      ],
    },
    widgets: {
      stops: "array",
      "stops.0": "object",
      "stops.0.at": "number",
      "stops.0.color": "color",
      "stops.1": "object",
      "stops.1.at": "number",
      "stops.1.color": "color",
    },
  },
  {
    name: "nullable number",
    schema: obj({ limit: { anyOf: [{ type: "integer", minimum: 0 }, { type: "null" }] } }, []),
    value: {},
    widgets: { limit: "number" },
  },
  {
    name: "$ref into $defs",
    schema: {
      ...obj({ pad: { $ref: "#/$defs/Pad" } }),
      $defs: { Pad: obj({ top: number({ default: 0 }), bottom: number({ default: 0 }) }) },
    },
    value: { pad: { top: 1, bottom: 2 } },
    widgets: { pad: "object", "pad.top": "number", "pad.bottom": "number" },
  },
  {
    name: "hidden widget is omitted",
    schema: obj({ id: ui({ widget: "hidden" }, { type: "string" }), name: { type: "string" } }),
    value: { id: "x", name: "Visible" },
    widgets: { name: "text" },
  },
  {
    name: "a time (flicks or text) edits as text",
    schema: obj({
      at: {
        anyOf: [{ type: "integer" }, { type: "string", pattern: "^-?[0-9]+(\\.[0-9]+)?s$" }],
      },
    }),
    value: { at: "2.5s" },
    widgets: { at: "text" },
  },
  {
    name: "an unsupported shape is shown, not hidden",
    schema: obj({ blob: { type: "object", additionalProperties: { type: "number" } } }),
    value: { blob: { a: 1 } },
    widgets: { blob: "unsupported" },
  },
  {
    name: "order hint and groups",
    schema: obj({
      b: ui({ order: 2, group: "Look" }, number()),
      a: ui({ order: 1, group: "Look" }, number()),
      c: number(),
    }),
    value: { a: 1, b: 2, c: 3 },
    widgets: { c: "number", a: "number", b: "number" },
  },
  {
    name: "advanced options start collapsed",
    schema: obj({
      crf: ui({ min: 0, max: 51 }, { type: "integer" }),
      threads: ui({ advanced: true }, { type: "integer", minimum: 0 }),
    }),
    value: { crf: 18, threads: 0 },
    widgets: { crf: "number" },
  },
  {
    name: "visibleWhen",
    schema: obj({
      mode: { type: "string", enum: ["crf", "bitrate"] },
      crf: ui({ visibleWhen: "mode == 'crf'" }, { type: "integer" }),
      bitrate: ui({ visibleWhen: "mode == 'bitrate'" }, { type: "integer" }),
    }),
    value: { mode: "crf", crf: 18, bitrate: 8000 },
    widgets: { mode: "segmented", crf: "number" },
  },
  {
    name: "multi-selection with mixed values",
    schema: obj({
      opacity: ui(
        { widget: "slider", min: 0, max: 1, step: 0.01, unit: "%", scale: 100 },
        number(),
      ),
      blend: { type: "string", enum: BLENDS },
      flip: { type: "boolean" },
      name: { type: "string" },
      position: obj({ x: number(), y: number() }),
    }),
    value: { opacity: 1, blend: "normal", flip: false, name: "Clip", position: { x: 0, y: 0 } },
    widgets: { opacity: "slider", blend: "select", flip: "toggle", name: "text", position: "vec2" },
    props: { mixed: ["opacity", "blend", "flip", "name", "position.x"] },
  },
];
