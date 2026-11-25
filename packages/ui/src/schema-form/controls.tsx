// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  useEffect,
  useRef,
  useState,
} from "react";
import { cx } from "../components/cx.ts";
import { IconButton } from "../components/IconButton.tsx";
import { NumberScrubber } from "../components/NumberScrubber.tsx";
import { clamp, decimalsOf, roundTo } from "../components/numberMath.ts";
import { SegmentedControl } from "../components/SegmentedControl.tsx";
import { Slider } from "../components/Slider.tsx";
import { Switch } from "../components/Switch.tsx";
import { TextField } from "../components/TextField.tsx";
import type { FieldNode } from "./compile.ts";
import type { AssetOption, FormOption, FormRenderers, FormStrings } from "./types.ts";

/** What every leaf control receives from its field. */
export interface ControlProps {
  node: FieldNode;
  label: string;
  /** `id` of the focusable element a `<label htmlFor>` points at. */
  id: string;
  /** `id` of the row's visible label, for controls that cannot take `htmlFor`. */
  labelId: string;
  value: unknown;
  mixed: boolean;
  disabled: boolean;
  strings: FormStrings;
  /** A streamed value while a gesture runs. */
  live: (value: unknown) => void;
  /** The final value of a gesture, or a one-shot change. */
  commit: (value: unknown) => void;
  /** Escape during a drag. */
  cancel: () => void;
}

const num = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;

/** The value a number control shows when the form has none (a nullable or unset field). */
function numberFallback(node: FieldNode): number {
  if (node.hasDefault && typeof node.default === "number") return node.default;
  return node.min !== undefined && node.min > 0 ? node.min : 0;
}

function displayConverters(node: FieldNode) {
  const { scale } = node;
  return {
    toDisplay: (v: number) => roundTo(v * scale, 10),
    fromDisplay: (d: number) => (node.integer ? Math.round(d / scale) : roundTo(d / scale, 10)),
    step: roundTo(node.step * scale, 10),
    min: node.min === undefined ? undefined : roundTo(node.min * scale, 10),
    max: node.max === undefined ? undefined : roundTo(node.max * scale, 10),
  };
}

export function NumberControl({
  node,
  label,
  value,
  mixed,
  disabled,
  live,
  commit,
  cancel,
}: ControlProps) {
  const c = displayConverters(node);
  const v = num(value, numberFallback(node));
  return (
    <NumberScrubber
      aria-label={label}
      value={c.toDisplay(v)}
      mixed={mixed}
      unit={node.unit}
      {...(c.min === undefined ? {} : { min: c.min })}
      {...(c.max === undefined ? {} : { max: c.max })}
      step={c.step}
      precision={node.precision}
      disabled={disabled}
      onChange={(d) => live(c.fromDisplay(d))}
      onCommit={(d) => commit(c.fromDisplay(d))}
      onCancel={cancel}
    />
  );
}

export function SliderControl({ node, label, value, mixed, disabled, live, commit }: ControlProps) {
  const c = displayConverters(node);
  const v = num(value, numberFallback(node));
  const unit = node.unit;
  const format = (d: number) => {
    if (mixed) return "—";
    const text = node.precision === undefined ? String(d) : d.toFixed(node.precision);
    return unit ? (unit === "%" || unit === "°" ? `${text}${unit}` : `${text} ${unit}`) : text;
  };
  return (
    <Slider
      label={label}
      labelHidden
      value={c.toDisplay(v)}
      min={c.min ?? 0}
      max={c.max ?? 100}
      step={c.step}
      format={format}
      disabled={disabled}
      onChange={(d) => live(c.fromDisplay(d))}
      onCommit={(d) => commit(c.fromDisplay(d))}
    />
  );
}

export function ToggleControl({
  label,
  labelId,
  id,
  value,
  mixed,
  disabled,
  commit,
  strings,
}: ControlProps) {
  return (
    <span className="cw-sf-toggle">
      <Switch
        id={id}
        aria-labelledby={labelId}
        checked={value === true}
        disabled={disabled}
        onCheckedChange={commit}
      />
      {mixed ? (
        <span className="cw-sf-mixed caption" role="status">
          {strings.mixed}
          <span className="cw-visually-hidden">{`: ${label}`}</span>
        </span>
      ) : null}
    </span>
  );
}

export function SegmentedControlField({
  node,
  label,
  value,
  mixed,
  disabled,
  commit,
}: ControlProps) {
  const options = node.options.map((o) => ({ value: String(o.value), label: o.label }));
  const current = mixed || value === undefined || value === null ? null : String(value);
  return (
    <SegmentedControl
      aria-label={label}
      options={options}
      value={current}
      disabled={disabled}
      onChange={(next) => commit(node.options.find((o) => String(o.value) === next)?.value ?? next)}
    />
  );
}

const MIXED = "__mixed";
const NONE = "__none";

export function SelectField({
  id,
  options,
  value,
  mixed,
  disabled,
  allowNone,
  strings,
  onPick,
  ariaLabel,
}: {
  id: string;
  options: readonly FormOption[];
  value: unknown;
  mixed: boolean;
  disabled: boolean;
  allowNone: boolean;
  strings: FormStrings;
  onPick: (value: string | number | null) => void;
  ariaLabel?: string | undefined;
}) {
  const index = options.findIndex((o) => o.value === value);
  const current = mixed ? MIXED : index >= 0 ? String(index) : allowNone ? NONE : MIXED;
  return (
    <select
      id={id}
      className="cw-sf-select body"
      aria-label={ariaLabel}
      value={current}
      disabled={disabled}
      onChange={(event) => {
        const next = event.target.value;
        if (next === NONE) onPick(null);
        else if (next !== MIXED) onPick(options[Number(next)]?.value ?? null);
      }}
    >
      {current === MIXED ? (
        <option value={MIXED} disabled>
          {mixed ? strings.mixed : "—"}
        </option>
      ) : null}
      {allowNone ? <option value={NONE}>{strings.none}</option> : null}
      {options.map((o, i) => (
        <option key={String(o.value)} value={String(i)}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function EnumSelectControl({
  node,
  id,
  label,
  value,
  mixed,
  disabled,
  commit,
  strings,
}: ControlProps) {
  return (
    <SelectField
      id={id}
      ariaLabel={label}
      options={node.options}
      value={value}
      mixed={mixed}
      disabled={disabled}
      allowNone={node.nullable}
      strings={strings}
      onPick={commit}
    />
  );
}

export function CurveControl(props: ControlProps & { renderers: FormRenderers }) {
  const { node, label, value, mixed, disabled, live, commit, renderers } = props;
  if (renderers.curve) {
    return (
      <>
        {renderers.curve({
          label,
          value,
          options: node.options,
          mixed,
          disabled,
          onChange: live,
          onCommit: commit,
        })}
      </>
    );
  }
  return <EnumSelectControl {...props} />;
}

export function AssetControl({
  node,
  id,
  value,
  mixed,
  disabled,
  commit,
  strings,
  assets,
}: ControlProps & { assets: readonly AssetOption[] | undefined }) {
  if (!assets)
    return (
      <TextControl {...({ node, id, value, mixed, disabled, commit, strings } as ControlProps)} />
    );
  const kind = node.hint.kind;
  const options = assets
    .filter((a) => kind === undefined || a.kind === undefined || a.kind === kind)
    .map((a) => ({ value: a.id, label: a.label }));
  return (
    <SelectField
      id={id}
      options={options}
      value={value}
      mixed={mixed}
      disabled={disabled}
      allowNone={!node.required || node.nullable}
      strings={strings}
      onPick={commit}
    />
  );
}

/** A text edit commits on Enter or blur, and Escape puts the old text back. */
function useTextDraft(shown: string, commitText: (text: string) => void) {
  const [draft, setDraft] = useState<string | null>(null);
  const finish = () => {
    if (draft !== null && draft !== shown) commitText(draft);
    setDraft(null);
  };
  return {
    text: draft ?? shown,
    onChange: setDraft,
    onBlur: finish,
    onKeyDown: (event: KeyboardEvent) => {
      if (event.key === "Enter" && !(event.target as HTMLElement).matches("textarea")) {
        event.preventDefault();
        finish();
      } else if (event.key === "Escape" && draft !== null) {
        event.preventDefault();
        setDraft(null);
      }
    },
  };
}

export function TextControl({ node, id, value, mixed, disabled, commit, strings }: ControlProps) {
  const shown = mixed || value === undefined || value === null ? "" : String(value);
  const edit = useTextDraft(shown, (text) => {
    const trimmed = text.trim();
    commit(node.textOrNumber && /^-?\d+$/.test(trimmed) ? Number(trimmed) : text);
  });
  return (
    <TextField
      id={id}
      value={edit.text}
      placeholder={mixed ? strings.mixed : node.hint.placeholder}
      disabled={disabled}
      numeric={node.textOrNumber}
      onChange={edit.onChange}
      onBlur={edit.onBlur}
      onKeyDown={edit.onKeyDown}
    />
  );
}

export function TextAreaControl({
  node,
  id,
  value,
  mixed,
  disabled,
  commit,
  strings,
}: ControlProps) {
  const shown = mixed || value === undefined || value === null ? "" : String(value);
  const edit = useTextDraft(shown, commit);
  return (
    <textarea
      id={id}
      className="cw-field body cw-sf-textarea"
      rows={node.hint.lines ?? 3}
      value={edit.text}
      placeholder={mixed ? strings.mixed : node.hint.placeholder}
      disabled={disabled}
      spellCheck={false}
      onChange={(event) => edit.onChange(event.target.value)}
      onBlur={edit.onBlur}
      onKeyDown={edit.onKeyDown}
    />
  );
}

/** Placeholder until the font picker (P3 text styling): the family name as text. */
export function FontControl(props: ControlProps) {
  return (
    <span className="cw-sf-font">
      <TextControl {...props} />
      <span className="cw-visually-hidden">{props.strings.fontNote}</span>
    </span>
  );
}

// ---------- Color ----------

interface Rgba {
  r: number;
  g: number;
  b: number;
  /** 0..1 */
  a: number;
  /** The text had an alpha pair (8 digits). */
  hasAlpha: boolean;
}

export function parseHex(text: string): Rgba | null {
  const m = /^#?([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(text.trim());
  if (!m?.[1]) return null;
  let h = m[1];
  if (h.length <= 4) h = [...h].map((c) => c + c).join("");
  const byte = (i: number) => Number.parseInt(h.slice(i, i + 2), 16);
  const hasAlpha = h.length === 8;
  return { r: byte(0), g: byte(2), b: byte(4), a: hasAlpha ? byte(6) / 255 : 1, hasAlpha };
}

const pair = (n: number) =>
  Math.round(clamp(n, 0, 255))
    .toString(16)
    .padStart(2, "0");

export function toHex(c: Pick<Rgba, "r" | "g" | "b" | "a">, withAlpha: boolean): string {
  return `#${pair(c.r)}${pair(c.g)}${pair(c.b)}${withAlpha ? pair(c.a * 255) : ""}`;
}

export function ColorControl({
  label,
  id,
  value,
  mixed,
  disabled,
  live,
  commit,
  cancel,
  strings,
}: ControlProps) {
  const parsed = typeof value === "string" ? parseHex(value) : null;
  const rgba: Rgba = parsed ?? { r: 0, g: 0, b: 0, a: 1, hasAlpha: false };
  const swatch = useRef<HTMLInputElement>(null);
  const latest = useRef(rgba);
  latest.current = rgba;
  const commitRef = useRef(commit);
  commitRef.current = commit;

  // The native `input` event streams while the picker is open, `change` closes it.
  useEffect(() => {
    const el = swatch.current;
    if (!el) return;
    const onChange = () => {
      const picked = parseHex(el.value);
      if (picked)
        commitRef.current(toHex({ ...picked, a: latest.current.a }, latest.current.hasAlpha));
    };
    el.addEventListener("change", onChange);
    return () => el.removeEventListener("change", onChange);
  }, []);

  const shown = mixed ? "" : toHex(rgba, rgba.hasAlpha);
  const edit = useTextDraft(shown, (text) => {
    const next = parseHex(text);
    if (next) commit(toHex(next, next.hasAlpha));
  });
  return (
    <span className="cw-sf-color">
      <input
        ref={swatch}
        id={id}
        type="color"
        className="cw-sf-swatch"
        aria-label={label}
        data-mixed={mixed ? "true" : undefined}
        value={toHex(rgba, false)}
        disabled={disabled}
        onChange={(event) => {
          const picked = parseHex(event.target.value);
          if (picked) live(toHex({ ...picked, a: rgba.a }, rgba.hasAlpha));
        }}
      />
      <TextField
        numeric
        aria-label={`${label} ${strings.hex}`}
        value={edit.text}
        placeholder={mixed ? strings.mixed : undefined}
        disabled={disabled}
        onChange={edit.onChange}
        onBlur={edit.onBlur}
        onKeyDown={edit.onKeyDown}
      />
      <NumberScrubber
        aria-label={`${label} ${strings.alpha.toLowerCase()}`}
        value={Math.round(rgba.a * 100)}
        mixed={mixed}
        unit="%"
        min={0}
        max={100}
        step={1}
        disabled={disabled}
        onChange={(pct) => live(toHex({ ...rgba, a: pct / 100 }, rgba.hasAlpha || pct < 100))}
        onCommit={(pct) => commit(toHex({ ...rgba, a: pct / 100 }, rgba.hasAlpha || pct < 100))}
        onCancel={cancel}
      />
    </span>
  );
}

// ---------- Angle ----------

function AngleDial({
  label,
  value,
  min,
  max,
  step,
  disabled,
  onLive,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  disabled: boolean;
  onLive: (v: number) => void;
  onCommit: (v: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const drag = useRef<number | null>(null);
  const current = useRef(value);
  current.current = value;

  const fromPointer = (event: PointerEvent): number => {
    const box = ref.current?.getBoundingClientRect();
    if (!box) return current.current;
    const dx = event.clientX - (box.left + box.width / 2);
    const dy = event.clientY - (box.top + box.height / 2);
    let deg = (Math.atan2(dx, -dy) * 180) / Math.PI; // 0 = up, clockwise
    if (event.shiftKey) deg = Math.round(deg / 15) * 15;
    // Continue from the current turn, so dragging across the top does not jump 360.
    const turns = Math.round((current.current - deg) / 360);
    return clamp(roundTo(deg + turns * 360, decimalsOf(step)), min, max);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const dir =
      event.key === "ArrowRight" || event.key === "ArrowUp"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowDown"
          ? -1
          : 0;
    if (dir === 0 || disabled) return;
    event.preventDefault();
    const next = clamp(
      roundTo(value + dir * step * (event.shiftKey ? 10 : 1), decimalsOf(step)),
      min,
      max,
    );
    onLive(next);
    onCommit(next);
  };

  return (
    <div
      ref={ref}
      role="slider"
      tabIndex={disabled ? -1 : 0}
      className="cw-sf-dial"
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      aria-valuetext={`${value}°`}
      aria-disabled={disabled ? "true" : undefined}
      onKeyDown={onKeyDown}
      onPointerDown={(event) => {
        if (disabled || event.button !== 0) return;
        event.currentTarget.setPointerCapture?.(event.pointerId);
        event.currentTarget.focus();
        drag.current = fromPointer(event);
        onLive(drag.current);
      }}
      onPointerMove={(event) => {
        if (drag.current === null) return;
        drag.current = fromPointer(event);
        onLive(drag.current);
      }}
      onPointerUp={(event) => {
        if (drag.current === null) return;
        event.currentTarget.releasePointerCapture?.(event.pointerId);
        const last = drag.current;
        drag.current = null;
        onCommit(last);
      }}
    >
      <span className="cw-sf-dial__needle" style={{ transform: `rotate(${value}deg)` }} />
    </div>
  );
}

export function AngleControl({
  node,
  label,
  value,
  mixed,
  disabled,
  live,
  commit,
  cancel,
  strings,
}: ControlProps) {
  const min = node.min ?? -360;
  const max = node.max ?? 360;
  const v = num(value, numberFallback(node));
  const step = node.hint.step ?? 1;
  return (
    <span className="cw-sf-angle">
      <AngleDial
        label={`${label} ${strings.dial}`}
        value={v}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onLive={live}
        onCommit={commit}
      />
      <NumberScrubber
        aria-label={label}
        value={v}
        mixed={mixed}
        unit={node.unit ?? "°"}
        min={min}
        max={max}
        step={step}
        precision={node.precision}
        disabled={disabled}
        onChange={live}
        onCommit={commit}
        onCancel={cancel}
      />
    </span>
  );
}

// ---------- Vec2 ----------

export function Vec2Control({
  node,
  label,
  value,
  mixed,
  disabled,
  live,
  commit,
  cancel,
  strings,
  mixedAxis,
}: ControlProps & { mixedAxis: (axis: "x" | "y") => boolean }) {
  const [linked, setLinked] = useState(node.linked);
  const origin = useRef<{ x: number; y: number } | null>(null);
  const read = (axis: "x" | "y"): number => {
    const v =
      node.axes === "object"
        ? (value as Record<string, unknown> | null | undefined)?.[axis]
        : (value as unknown[] | null | undefined)?.[axis === "x" ? 0 : 1];
    return num(v, 0);
  };
  const cur = { x: read("x"), y: read("y") };
  const build = (x: number, y: number): unknown =>
    node.axes === "array" ? [x, y] : { ...(value as object | null | undefined), x, y };
  const next = (axis: "x" | "y", n: number): unknown => {
    const o = origin.current ?? cur;
    if (!linked) return axis === "x" ? build(n, cur.y) : build(cur.x, n);
    const from = axis === "x" ? o.x : o.y;
    const ratio = from === 0 ? null : n / from;
    const other = axis === "x" ? o.y : o.x;
    const follow = ratio === null ? n : roundTo(other * ratio, 6);
    return axis === "x" ? build(n, follow) : build(follow, n);
  };
  const c = displayConverters(node);
  const axisField = (axis: "x" | "y") => (
    <NumberScrubber
      label={axis.toUpperCase()}
      aria-label={`${label} ${axis.toUpperCase()}`}
      value={c.toDisplay(cur[axis])}
      mixed={mixed || mixedAxis(axis)}
      unit={node.unit}
      {...(c.min === undefined ? {} : { min: c.min })}
      {...(c.max === undefined ? {} : { max: c.max })}
      step={c.step}
      precision={node.precision}
      disabled={disabled}
      onChange={(d) => {
        origin.current ??= cur;
        live(next(axis, c.fromDisplay(d)));
      }}
      onCommit={(d) => {
        const out = next(axis, c.fromDisplay(d));
        origin.current = null;
        commit(out);
      }}
      onCancel={() => {
        origin.current = null;
        cancel();
      }}
    />
  );
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
    <span role="group" aria-label={label} className="cw-sf-vec2">
      {axisField("x")}
      <IconButton
        size="sm"
        icon={linked ? "link" : "unlink"}
        label={strings.linkAxesFor(label)}
        pressed={linked}
        disabled={disabled}
        onClick={() => setLinked(!linked)}
      />
      {axisField("y")}
    </span>
  );
}

// ---------- Unsupported ----------

export function UnsupportedControl({
  value,
  strings,
}: {
  value: unknown;
  strings: FormStrings;
}): ReactNode {
  return (
    <span className={cx("cw-sf-unsupported", "caption")} title={strings.unsupported}>
      <code>{value === undefined ? "—" : JSON.stringify(value)}</code>
    </span>
  );
}
