// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { FIXTURES, hex } from "./fixtures.ts";
import "./schema-form.css";
import { SchemaForm, type SchemaFormProps } from "./SchemaForm.tsx";
import type { ChangeMeta, FieldKeyframe, JsonSchema } from "./types.ts";
import { setPath } from "./values.ts";

export default { title: "Forms/SchemaForm", component: SchemaForm, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  width: "var(--layer-width-lg)",
} as const;

/** A form bound to state, as an inspector is bound to engine state. */
function Bound({
  schema,
  initial,
  ...rest
}: { schema: JsonSchema; initial: unknown } & Partial<SchemaFormProps>) {
  const [value, setValue] = useState(initial);
  return (
    <SchemaForm
      aria-label="Properties"
      schema={schema}
      value={value}
      onChange={(path: string, next: unknown, _meta: ChangeMeta) =>
        setValue((v: unknown) => setPath(v, path, next))
      }
      {...rest}
    />
  );
}

const number = (extra: JsonSchema = {}): JsonSchema => ({ type: "number", ...extra });

const clipSchema: JsonSchema = {
  type: "object",
  properties: {
    position: {
      type: "object",
      properties: { x: number(), y: number() },
      "x-ui": { group: "Transform", step: 1, unit: "px", keyframeable: true },
    },
    scale: {
      type: "array",
      items: number(),
      minItems: 2,
      maxItems: 2,
      default: [1, 1],
      "x-ui": {
        widget: "vec2",
        group: "Transform",
        linked: true,
        min: 0.01,
        max: 10,
        step: 0.01,
        unit: "%",
        scale: 100,
        keyframeable: true,
      },
    },
    rotation: {
      type: "number",
      default: 0,
      "x-ui": { widget: "angle", group: "Transform", min: -360, max: 360, keyframeable: true },
    },
    opacity: {
      type: "number",
      default: 1,
      "x-ui": {
        widget: "slider",
        group: "Compositing",
        min: 0,
        max: 1,
        step: 0.01,
        unit: "%",
        scale: 100,
        keyframeable: true,
      },
    },
    blend: {
      type: "string",
      enum: ["normal", "multiply", "screen", "overlay", "add", "darken"],
      default: "normal",
      "x-ui": { group: "Compositing" },
    },
    fit: {
      type: "string",
      enum: ["fit", "fill", "stretch"],
      default: "fit",
      "x-ui": { group: "Compositing" },
    },
    flipH: {
      type: "boolean",
      default: false,
      "x-ui": { group: "Compositing", label: "Flip horizontal" },
    },
    background: {
      type: "string",
      default: hex("000000"),
      "x-ui": { widget: "color", group: "Compositing" },
    },
  },
};

const noop = () => {};
const kf = (state: FieldKeyframe["state"]): FieldKeyframe => ({
  state,
  onToggle: noop,
  onPrev: noop,
  onNext: noop,
});

export const ClipProperties = {
  render: () => (
    <div style={col}>
      <Bound
        schema={clipSchema}
        initial={{
          position: { x: 120, y: -40 },
          scale: [1.5, 1.5],
          rotation: 15,
          opacity: 0.8,
          blend: "screen",
          fit: "fit",
          flipH: false,
          background: hex("000000"),
        }}
        keyframes={{
          position: kf("on"),
          scale: kf("elsewhere"),
          rotation: kf("off"),
          opacity: kf("off"),
        }}
        onResetGroup={noop}
      />
    </div>
  ),
};

const exportSchema: JsonSchema = {
  type: "object",
  properties: {
    container: { type: "string", enum: ["mp4", "mov", "webm"], default: "mp4" },
    rate_control: {
      type: "string",
      enum: ["crf", "cbr", "vbr"],
      default: "crf",
      "x-ui": { label: "Rate control" },
    },
    crf: {
      type: "integer",
      minimum: 0,
      maximum: 51,
      default: 18,
      "x-ui": { visibleWhen: "rate_control == 'crf'", label: "Quality (CRF)" },
    },
    bitrate: {
      type: "integer",
      minimum: 100,
      default: 8000,
      "x-ui": { visibleWhen: "rate_control != 'crf'", unit: "kb/s", step: 100 },
    },
    two_pass: { type: "boolean", default: false, "x-ui": { visibleWhen: "rate_control == 'vbr'" } },
    keyint: {
      type: "integer",
      minimum: 1,
      default: 60,
      "x-ui": { advanced: true, label: "Keyframe interval" },
    },
    threads: { type: "integer", minimum: 0, default: 0, "x-ui": { advanced: true } },
    note: {
      type: "string",
      "x-ui": { lines: 3, placeholder: "Shown in the export queue", advanced: true },
    },
  },
};

export const ExportOptions = {
  render: () => (
    <div style={col}>
      <Bound
        schema={exportSchema}
        initial={{
          container: "mp4",
          rate_control: "vbr",
          crf: 18,
          bitrate: 8000,
          two_pass: true,
          keyint: 60,
          threads: 0,
          note: "",
        }}
      />
    </div>
  ),
};

/** Values the host refused (the engine's `Invalid` pointers): the reason under each field. */
export const RefusedValues = {
  render: () => (
    <div style={col}>
      <Bound
        schema={exportSchema}
        initial={{
          container: "mp4",
          rate_control: "cbr",
          crf: 18,
          bitrate: 80000,
          two_pass: true,
          keyint: 60,
          threads: 0,
          note: "",
        }}
        errors={{
          rate_control: "This hardware encoder has no constant bitrate; use VBR or software.",
          bitrate: "At most 50000 kb/s.",
        }}
      />
    </div>
  ),
};

export const MultiSelection = {
  render: () => (
    <div style={col}>
      <Bound
        schema={clipSchema}
        initial={{
          position: { x: 0, y: 0 },
          scale: [1, 1],
          rotation: 0,
          opacity: 1,
          blend: "normal",
          fit: "fit",
          flipH: false,
          background: hex("000000"),
        }}
        mixed={["position.x", "opacity", "blend", "fit", "flipH", "background"]}
      />
    </div>
  ),
};

export const Disabled = {
  render: () => (
    <div style={col}>
      <Bound
        schema={clipSchema}
        initial={{
          position: { x: 1, y: 2 },
          scale: [2, 2],
          rotation: 30,
          opacity: 0.5,
          blend: "add",
          fit: "fill",
          flipH: true,
          background: hex("336699"),
        }}
        disabled
      />
    </div>
  ),
};

/** Every fixture the tests check, as one page: the widget catalogue. */
export const Gallery = {
  render: () => (
    <div style={{ ...col, gap: "var(--space-4)" }}>
      {FIXTURES.map((f) => (
        <section
          key={f.name}
          style={{ display: "flex", flexDirection: "column", gap: "var(--space-1)" }}
        >
          <span className="caption" style={{ color: "var(--text-muted)" }}>
            {f.name}
          </span>
          <Bound schema={f.schema} initial={f.value} {...f.props} aria-label={f.name} />
        </section>
      ))}
    </div>
  ),
};
