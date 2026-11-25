// SPDX-License-Identifier: GPL-3.0-or-later
import type { FormOption, JsonSchema, UiHints, WidgetName } from "./types.ts";
import { humanize, joinPath } from "./values.ts";
import { parseVisibleWhen, type VisibleExpr } from "./visibleWhen.ts";

export type WidgetKind =
  | Exclude<WidgetName, "hidden">
  | "object"
  | "array"
  | "tuple"
  | "unsupported";

/** One renderable field, resolved from the schema and its `x-ui` hints. */
export interface FieldNode {
  path: string;
  key: string;
  label: string;
  description: string | undefined;
  widget: WidgetKind;
  hint: UiHints;
  required: boolean;
  nullable: boolean;
  hasDefault: boolean;
  default: unknown;
  integer: boolean;
  min: number | undefined;
  max: number | undefined;
  step: number;
  /** Display = value x scale. */
  scale: number;
  unit: string | undefined;
  precision: number | undefined;
  options: readonly FormOption[];
  /** `object`: the fields, in display order. */
  children: readonly FieldNode[];
  keyframeable: boolean;
  advanced: boolean;
  group: string | undefined;
  order: number;
  visibleWhen: VisibleExpr | null;
  /** `vec2`: the value is `{x, y}` or `[x, y]`. */
  axes: "object" | "array";
  linked: boolean;
  /** `array` and `tuple`: the node of element `index`, labelled `label`. */
  itemAt: ((index: number, label: string) => FieldNode) | undefined;
  minItems: number | undefined;
  maxItems: number | undefined;
  /** `tuple`: the fixed length. */
  tupleLength: number;
  /** `text` for a value that is a number or a string (a time: flicks or "2.5s"). */
  textOrNumber: boolean;
  /** Seed for a new array element. */
  seed: () => unknown;
}

const MAX_DEPTH = 8;

/** A step that gives about 100 notches over the range; 0.1 when the range is open. */
function niceStep(integer: boolean, min: number | undefined, max: number | undefined): number {
  if (integer) return 1;
  if (min === undefined || max === undefined || !(max > min)) return 0.1;
  return 10 ** Math.floor(Math.log10((max - min) / 100));
}

type Defs = Readonly<Record<string, JsonSchema>>;

const isNullSchema = (s: JsonSchema) => s.type === "null" || (s.const === null && "const" in s);

/** Follows `$ref` and single `allOf`; the referring schema's own keys (description, default) win. */
function resolve(schema: JsonSchema, defs: Defs, depth = 0): JsonSchema {
  let s = schema;
  for (let i = 0; i < 16; i++) {
    if (s.$ref) {
      const { $ref, ...rest } = s;
      const name = $ref.split("/").pop() ?? "";
      const target = defs[name];
      if (!target) return rest;
      s = { ...target, ...rest, "x-ui": { ...target["x-ui"], ...rest["x-ui"] } };
    } else if (s.allOf && s.allOf.length === 1 && s.allOf[0]) {
      const { allOf, ...rest } = s;
      const inner = resolve(allOf[0] as JsonSchema, defs, depth + 1);
      s = { ...inner, ...rest, "x-ui": { ...inner["x-ui"], ...rest["x-ui"] } };
    } else {
      break;
    }
  }
  return s;
}

interface Normalized {
  schema: JsonSchema;
  nullable: boolean;
}

/** Strips `null` out of `type: [T, "null"]` and `anyOf: [T, {type: "null"}]`. */
function normalize(schema: JsonSchema, defs: Defs): Normalized {
  let s = resolve(schema, defs);
  let nullable = false;
  if (Array.isArray(s.type)) {
    const types = (s.type as readonly string[]).filter((t) => t !== "null");
    nullable = types.length !== s.type.length;
    const { type: _type, ...rest } = s;
    const only = types[0];
    s =
      types.length === 1 && only
        ? { ...rest, type: only }
        : types.length === 0
          ? rest
          : { ...rest, type: types };
  }
  for (const key of ["anyOf", "oneOf"] as const) {
    const variants = s[key];
    if (!variants?.some((v) => isNullSchema(resolve(v, defs)))) continue;
    nullable = true;
    const rest = variants.filter((v) => !isNullSchema(resolve(v, defs)));
    const { [key]: _drop, ...outer } = s;
    if (rest.length === 1 && rest[0]) {
      const inner = resolve(rest[0], defs);
      s = { ...inner, ...outer, "x-ui": { ...inner["x-ui"], ...outer["x-ui"] } };
    } else {
      s = { ...outer, [key]: rest };
    }
  }
  return { schema: s, nullable };
}

function typeOf(s: JsonSchema): string | undefined {
  if (typeof s.type === "string") return s.type;
  if (Array.isArray(s.type) && s.type.length === 1) return s.type[0];
  if (s.properties) return "object";
  return undefined;
}

function enumOptions(s: JsonSchema, defs: Defs): FormOption[] | null {
  if (Array.isArray(s.enum) && s.enum.length > 0) {
    return s.enum.map((v) => ({ value: v as string | number, label: humanize(String(v)) }));
  }
  const variants = s.oneOf ?? s.anyOf;
  if (!variants || variants.length === 0) return null;
  const options: FormOption[] = [];
  for (const raw of variants) {
    const v = resolve(raw, defs);
    const value =
      "const" in v ? v.const : Array.isArray(v.enum) && v.enum.length === 1 ? v.enum[0] : undefined;
    if (value === undefined) return null;
    options.push({ value: value as string | number, label: v.title ?? humanize(String(value)) });
  }
  return options;
}

const isNumberType = (t: string | undefined) => t === "number" || t === "integer";

export interface CompileOptions {
  /** Hints by dot path; they override the schema's own `x-ui`. */
  hints?: Readonly<Record<string, UiHints>> | undefined;
}

/** Resolves a schema (and `x-ui` hints) into the field tree the form renders. */
export function compileSchema(schema: JsonSchema, options: CompileOptions = {}): FieldNode[] {
  const defs: Defs = { ...schema.definitions, ...schema.$defs };
  const hints = options.hints ?? {};
  const root = normalize(schema, defs).schema;
  if (typeOf(root) === "object" && root.properties) {
    return compileChildren(root, "", defs, hints, 0);
  }
  return [compileNode(schema, "", "value", false, defs, hints, 0)];
}

function compileChildren(
  s: JsonSchema,
  parent: string,
  defs: Defs,
  hints: Readonly<Record<string, UiHints>>,
  depth: number,
): FieldNode[] {
  const required = new Set(s.required ?? []);
  const nodes = Object.entries(s.properties ?? {}).map(([key, child], index) => ({
    node: compileNode(child, joinPath(parent, key), key, required.has(key), defs, hints, depth + 1),
    index,
  }));
  return nodes
    .filter(({ node }) => node.hint.widget !== "hidden")
    .sort((a, b) => a.node.order - b.node.order || a.index - b.index)
    .map(({ node }) => node);
}

function compileNode(
  raw: JsonSchema,
  path: string,
  key: string,
  required: boolean,
  defs: Defs,
  hints: Readonly<Record<string, UiHints>>,
  depth: number,
  labelOverride?: string,
): FieldNode {
  const { schema: s, nullable } = normalize(raw, defs);
  const hint: UiHints = { ...s["x-ui"], ...hints[path] };
  const type = typeOf(s);
  const options = enumOptions(s, defs) ?? [];
  const integer = type === "integer";
  const min = hint.min ?? s.minimum;
  const max = hint.max ?? s.maximum;
  const scale = hint.scale ?? 1;
  const base: FieldNode = {
    path,
    key,
    label: labelOverride ?? hint.label ?? s.title ?? humanize(key),
    description: s.description,
    widget: "unsupported",
    hint,
    required,
    nullable,
    hasDefault: "default" in s && s.default !== undefined && s.default !== null,
    default: s.default,
    integer,
    min,
    max,
    step: hint.step ?? s.multipleOf ?? niceStep(integer, min, max),
    scale,
    unit: hint.unit,
    precision: hint.precision,
    options,
    children: [],
    keyframeable: hint.keyframeable === true,
    advanced: hint.advanced === true,
    group: hint.group,
    order: hint.order ?? 0,
    visibleWhen: hint.visibleWhen ? parseVisibleWhen(hint.visibleWhen) : null,
    axes: "object",
    linked: hint.linked === true,
    itemAt: undefined,
    minItems: s.minItems,
    maxItems: s.maxItems,
    tupleLength: 0,
    textOrNumber: false,
    seed: () => "",
  };
  const finish = (patch: Partial<FieldNode>): FieldNode => {
    const node = { ...base, ...patch };
    node.seed = () => seedFor(node);
    return node;
  };

  if (depth > MAX_DEPTH) return finish({});
  const forced = hint.widget && hint.widget !== "hidden" ? hint.widget : undefined;

  // 1. Enums (also `oneOf` of `const`s).
  if (options.length > 0 && !(type === "object" || type === "array")) {
    let widget: WidgetKind =
      options.length <= 4 && options.every((o) => o.label.length <= 12) ? "segmented" : "select";
    if (forced === "select" || forced === "segmented" || forced === "curve") widget = forced;
    return finish({ widget });
  }

  // 2. Objects: a vec2, or a group of fields.
  if (type === "object" && s.properties) {
    const keys = Object.keys(s.properties);
    const isXY =
      keys.length === 2 &&
      keys.includes("x") &&
      keys.includes("y") &&
      isNumberType(typeOf(normalize(s.properties.x as JsonSchema, defs).schema)) &&
      isNumberType(typeOf(normalize(s.properties.y as JsonSchema, defs).schema));
    if (forced === "vec2" || (isXY && forced === undefined)) {
      const xs = normalize((s.properties.x ?? {}) as JsonSchema, defs).schema;
      return finish({
        widget: "vec2",
        axes: "object",
        integer: typeOf(xs) === "integer",
        min: min ?? xs.minimum,
        max: max ?? xs.maximum,
        step: hint.step ?? niceStep(typeOf(xs) === "integer", min ?? xs.minimum, max ?? xs.maximum),
      });
    }
    return finish({
      widget: "object",
      children: compileChildren(s, path, defs, hints, depth),
    });
  }

  // 3. Arrays: a vec2 as `[x, y]`, a fixed-length tuple, or a list.
  if (type === "array") {
    const items = (Array.isArray(s.items) ? undefined : s.items) as JsonSchema | undefined;
    const prefix =
      s.prefixItems ?? (Array.isArray(s.items) ? (s.items as readonly JsonSchema[]) : undefined);
    const itemType = items ? typeOf(normalize(items, defs).schema) : undefined;
    const fixed =
      prefix?.length ?? (s.minItems !== undefined && s.minItems === s.maxItems ? s.minItems : 0);
    const numeric =
      prefix !== undefined
        ? prefix.every((p) => isNumberType(typeOf(normalize(p, defs).schema)))
        : isNumberType(itemType);
    if (fixed > 0 && numeric) {
      const first = prefix?.[0] ?? items;
      const fs = first ? normalize(first, defs).schema : {};
      const patch = {
        integer: typeOf(fs) === "integer",
        min: min ?? fs.minimum,
        max: max ?? fs.maximum,
        step: hint.step ?? niceStep(typeOf(fs) === "integer", min ?? fs.minimum, max ?? fs.maximum),
        tupleLength: fixed,
      };
      if (fixed === 2 && forced !== "angle")
        return finish({ ...patch, widget: "vec2", axes: "array" });
      return finish({
        ...patch,
        widget: "tuple",
        itemAt: (index, label) => {
          const itemSchema = prefix?.[index] ?? (items as JsonSchema);
          return compileNode(
            {
              ...itemSchema,
              "x-ui": {
                ...(min === undefined ? {} : { min }),
                ...(max === undefined ? {} : { max }),
                ...(hint.step === undefined ? {} : { step: hint.step }),
                ...(hint.unit === undefined ? {} : { unit: hint.unit }),
                ...itemSchema["x-ui"],
              },
            },
            joinPath(path, index),
            String(index),
            true,
            defs,
            hints,
            depth + 1,
            label,
          );
        },
      });
    }
    if (items && !prefix) {
      return finish({
        widget: "array",
        itemAt: (index, label) =>
          compileNode(
            items,
            joinPath(path, index),
            String(index),
            true,
            defs,
            hints,
            depth + 1,
            label,
          ),
      });
    }
    return finish({});
  }

  // 4. Scalars.
  if (isNumberType(type)) {
    let widget: WidgetKind = "number";
    if (forced === "slider" && min !== undefined && max !== undefined) widget = "slider";
    else if (forced === "angle") widget = "angle";
    return finish({ widget });
  }
  if (type === "boolean") return finish({ widget: "toggle" });
  if (type === "string") {
    let widget: WidgetKind = "text";
    if (
      forced === "color" ||
      forced === "font" ||
      forced === "asset" ||
      forced === "textarea" ||
      forced === "text"
    ) {
      widget = forced;
    } else if ((hint.lines ?? 1) > 1) {
      widget = "textarea";
    }
    return finish({ widget });
  }

  // 5. Unions of a number and a string (a time) edit as text; anything else has no editor yet.
  const variants = (s.anyOf ?? s.oneOf ?? []).map((v) => typeOf(normalize(v, defs).schema));
  if (variants.length > 0 && variants.every((t) => isNumberType(t) || t === "string")) {
    return finish({ widget: "text", textOrNumber: true });
  }
  return finish({});
}

/** The value a new array element starts from. */
function seedFor(node: FieldNode): unknown {
  if (node.hasDefault) return node.default;
  switch (node.widget) {
    case "toggle":
      return false;
    case "number":
    case "slider":
    case "angle":
      return node.min !== undefined && node.min > 0 ? node.min : 0;
    case "segmented":
    case "select":
    case "curve":
      return node.options[0]?.value ?? "";
    case "vec2":
      return node.axes === "array" ? [0, 0] : { x: 0, y: 0 };
    case "tuple":
      return Array.from({ length: node.tupleLength }, () => 0);
    case "array":
      return [];
    case "object":
      return Object.fromEntries(node.children.map((c) => [c.key, c.seed()]));
    default:
      return "";
  }
}

/** The default of a node: the schema's, or the one the host passed for its path. */
export function defaultOf(
  node: FieldNode,
  overrides: Readonly<Record<string, unknown>> | undefined,
): { has: boolean; value: unknown } {
  if (overrides && node.path in overrides) return { has: true, value: overrides[node.path] };
  return { has: node.hasDefault, value: node.default };
}
