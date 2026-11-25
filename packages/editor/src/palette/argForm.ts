// SPDX-License-Identifier: GPL-3.0-or-later
// The argument form of a palette command (WP-2H.04), generated from the command's input schema:
// one field per property, with a control by type. It is the palette's small stand-in for the
// schema-driven inspector's `SchemaForm` (WP-2H.02); that renderer takes its place when it
// lands, this model (fields in, JSON arguments out) stays.
import type { TKey } from "../i18n/index.ts";
import type { JsonSchema } from "./catalog.ts";

export type FieldKind = "text" | "number" | "boolean" | "select" | "json";

export interface FieldOption {
  readonly value: string;
  readonly label: string;
}

export interface Field {
  /** The property name (the key in the arguments). */
  readonly name: string;
  readonly label: string;
  readonly description?: string;
  readonly required: boolean;
  readonly kind: FieldKind;
  /** A number field takes only whole numbers. */
  readonly integer: boolean;
  /** A text field whose schema also allows a number (a time): digits are sent as a number. */
  readonly numeric: boolean;
  readonly options: readonly FieldOption[];
  /** The property that can be given instead of this required one (`items` for `item`). */
  readonly alternative?: string;
}

/** What a field holds while it is edited: text, or on/off. */
export type FieldValues = Readonly<Record<string, string | boolean>>;

/** `source_in` as "Source in". */
export function humanize(name: string): string {
  const spaced = name.replaceAll("_", " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

function deref(root: JsonSchema, schema: JsonSchema): JsonSchema {
  const ref = schema.$ref;
  if (ref === undefined) return schema;
  const name = ref.split("/").pop() ?? "";
  return root.$defs?.[name] ?? root.definitions?.[name] ?? schema;
}

/** The type names a schema allows, looking through `anyOf` and `oneOf`. */
function typesOf(root: JsonSchema, schema: JsonSchema): Set<string> {
  const s = deref(root, schema);
  const out = new Set<string>();
  if (typeof s.type === "string") out.add(s.type);
  else if (Array.isArray(s.type)) for (const t of s.type) out.add(t);
  for (const branch of [...(s.anyOf ?? []), ...(s.oneOf ?? [])]) {
    for (const t of typesOf(root, branch)) out.add(t);
  }
  out.delete("null");
  return out;
}

function enumOf(root: JsonSchema, schema: JsonSchema): readonly unknown[] | undefined {
  const s = deref(root, schema);
  if (s.enum !== undefined) return s.enum;
  const branches = s.anyOf ?? s.oneOf;
  if (branches === undefined) return undefined;
  const values: unknown[] = [];
  for (const branch of branches) {
    const inner = deref(root, branch);
    if (inner.enum === undefined) return undefined;
    values.push(...inner.enum);
  }
  return values;
}

function fieldOf(root: JsonSchema, name: string, schema: JsonSchema, required: boolean): Field {
  const s = deref(root, schema);
  const common = {
    name,
    label: humanize(name),
    required,
    integer: false,
    numeric: false,
    ...(s.description !== undefined && { description: s.description }),
  };
  const values = enumOf(root, s);
  const names = values?.every((v) => typeof v === "string") ? (values as string[]) : undefined;
  if (names !== undefined) {
    const labels = s["x-enum-labels"] ?? {};
    return {
      ...common,
      kind: "select",
      options: names.map((v) => ({ value: v, label: labels[v] ?? humanize(v) })),
    };
  }
  const types = typesOf(root, s);
  if (types.size === 1 && types.has("boolean")) return { ...common, kind: "boolean", options: [] };
  if (types.size === 1 && (types.has("integer") || types.has("number"))) {
    return { ...common, kind: "number", integer: types.has("integer"), options: [] };
  }
  if (
    types.size > 0 &&
    [...types].every((t) => t === "string" || t === "integer" || t === "number")
  ) {
    return {
      ...common,
      kind: "text",
      numeric: types.has("integer") || types.has("number"),
      options: [],
    };
  }
  // Objects, arrays and unions the form has no control for: JSON text.
  return { ...common, kind: "json", options: [] };
}

/**
 * Arguments a command takes one of, the engine refusing both and neither (`clip.split` and
 * `item.move` take `item` or `items`, `item.move` takes `start` or `delta`). The schema has
 * both optional; the form asks for the first, and giving the second instead also does.
 */
const ALTERNATIVES: readonly (readonly [string, string])[] = [
  ["item", "items"],
  ["start", "delta"],
];

/** The alternatives a schema has, as (asked, instead) pairs. */
function choicesOf(schema: JsonSchema): (readonly [string, string])[] {
  const props = schema.properties ?? {};
  const required = new Set(schema.required ?? []);
  return ALTERNATIVES.filter(
    ([a, b]) => a in props && b in props && !required.has(a) && !required.has(b),
  );
}

/** What running the command needs: the schema's required names, and one of each alternative. */
function requiredOf(schema: JsonSchema): string[] {
  return [...(schema.required ?? []), ...choicesOf(schema).map(([a]) => a)];
}

/** The fields of an input schema, required ones first, then in the schema's order. */
export function fieldsOf(schema: JsonSchema): Field[] {
  const required = new Set(requiredOf(schema));
  const instead = new Map(choicesOf(schema));
  const fields = Object.entries(schema.properties ?? {}).map(([name, prop]) => {
    const field = fieldOf(schema, name, prop, required.has(name));
    const alternative = instead.get(name);
    return alternative === undefined ? field : { ...field, alternative };
  });
  return [...fields.filter((f) => f.required), ...fields.filter((f) => !f.required)];
}

/** Whether running the command needs arguments the person has to give. */
export function needsArguments(schema: JsonSchema): boolean {
  return requiredOf(schema).length > 0;
}

/** The starting values of a form: a field's prefilled value, else empty (a switch is off). */
export function initialValues(
  fields: readonly Field[],
  prefilled: Readonly<Record<string, unknown>>,
): FieldValues {
  const out: Record<string, string | boolean> = {};
  for (const f of fields) {
    const given = prefilled[f.name];
    if (f.kind === "boolean") out[f.name] = given === true;
    else if (f.kind === "json") {
      out[f.name] = given === undefined ? "" : JSON.stringify(given);
    } else out[f.name] = given === undefined ? "" : String(given);
  }
  return out;
}

export type FormResult =
  | { readonly ok: true; readonly args: Record<string, unknown> }
  | { readonly ok: false; readonly errors: Readonly<Record<string, TKey>> };

/** The arguments a form holds, or what is wrong with each field. */
export function toArguments(fields: readonly Field[], values: FieldValues): FormResult {
  const args: Record<string, unknown> = {};
  const errors: Record<string, TKey> = {};
  for (const f of fields) {
    const v = values[f.name];
    if (f.kind === "boolean") {
      if (v === true || f.required) args[f.name] = v === true;
      continue;
    }
    const text = typeof v === "string" ? v.trim() : "";
    if (text === "") {
      const other = f.alternative === undefined ? undefined : values[f.alternative];
      const givenInstead = typeof other === "string" ? other.trim() !== "" : other === true;
      if (f.required && !givenInstead) errors[f.name] = "palette.form.required";
      continue;
    }
    if (f.kind === "number" || (f.kind === "text" && f.numeric && /^-?\d+(\.\d+)?$/.test(text))) {
      const n = Number(text);
      if (!Number.isFinite(n) || (f.integer && !Number.isInteger(n))) {
        errors[f.name] = f.integer ? "palette.form.notWhole" : "palette.form.notNumber";
        continue;
      }
      args[f.name] = n;
    } else if (f.kind === "json") {
      try {
        args[f.name] = JSON.parse(text) as unknown;
      } catch {
        errors[f.name] = "palette.form.notJson";
      }
    } else args[f.name] = text;
  }
  return Object.keys(errors).length > 0 ? { ok: false, errors } : { ok: true, args };
}

/** What the editor knows that can fill a field before the person types. */
export interface Prefill {
  /** The selected timeline items. */
  readonly items: readonly string[];
  /** The playhead in flicks. */
  readonly playhead: number | null;
}

/**
 * Arguments the context answers: `item` / `items` from the selected clips (an ID pattern names
 * the kind), `at` from the playhead. A command that takes `item` or `items` gets `item` for one
 * selected clip and `items` for several, never both. Anything else is left for the person.
 */
export function prefillFor(schema: JsonSchema, context: Prefill): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const either = choicesOf(schema).some(([a]) => a === "item");
  const count = context.items.length;
  for (const [name, prop] of Object.entries(schema.properties ?? {})) {
    const s = deref(schema, prop);
    const itemPattern = (p: JsonSchema | undefined): boolean =>
      p?.pattern?.startsWith("^itm_") === true;
    const isArray = s.type === "array" || (Array.isArray(s.type) && s.type.includes("array"));
    if (
      name === "item" &&
      itemPattern(s) &&
      context.items[0] !== undefined &&
      !(either && count > 1)
    ) {
      out[name] = context.items[0];
    } else if (name === "items" && isArray && itemPattern(s.items) && count > (either ? 1 : 0)) {
      out[name] = [...context.items];
    } else if (name === "at" && context.playhead !== null) {
      out[name] = context.playhead;
    }
  }
  return out;
}

/** Whether every required property has a prefilled value (the command can run at once). */
export function fullyPrefilled(
  schema: JsonSchema,
  prefilled: Readonly<Record<string, unknown>>,
): boolean {
  const instead = new Map(choicesOf(schema));
  return requiredOf(schema).every((name) => {
    const other = instead.get(name);
    return prefilled[name] !== undefined || (other !== undefined && prefilled[other] !== undefined);
  });
}
