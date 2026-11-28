// SPDX-License-Identifier: GPL-3.0-or-later
// The Inspector's Video sections, driven by the `item.set_props` schema (WP-2H.03a).
//
// The engine's input schema carries `x-ui` hints (`group`, `keyframeable`, `unit`, `scale`; see
// packages/ui/SCHEMA_HINTS.md). This file turns it into sections without naming a single
// property: a property of `props` with `x-ui.group` `transform`, `crop`, `flip` or `compositing`
// (or none) belongs to the Video part, and a new property in the schema gets a control with no
// UI code change. `clip` and `audio` groups are other sections' (Clip info, WP-2H.03b).
import type { ClipItem as Item, ItemProps, PropId, ResetGroup } from "@cutweave/protocol";
import type { KeyframeState } from "@cutweave/ui";
import { getPath, type JsonSchema, setPath, type UiHints } from "@cutweave/ui";

/** Groups whose properties are not part of the Video sections. */
const OTHER_SECTIONS: ReadonlySet<string> = new Set(["clip", "audio"]);

/** The engine's reset groups of the video part (`item.reset_props`). */
const RESET_GROUPS: ReadonlySet<string> = new Set<ResetGroup>([
  "transform",
  "crop",
  "flip",
  "compositing",
  "audio",
]);

/** The group of a property the schema gives no `x-ui.group`. */
export const OTHER_VIDEO_GROUP = "video";

/** A part of the Video section: one form, and what it edits. */
export interface VideoSection {
  /** `transform`, `crop`, `flip`, `compositing`, or `video` for a property with no group. */
  readonly id: string;
  /** The engine's reset group for the section, when it has one. */
  readonly resetGroup: ResetGroup | undefined;
  /**
   * Where the section's fields sit inside `props`: `transform` for an object property shown as
   * the whole section, `""` when the section holds several top-level properties.
   */
  readonly prefix: string;
  /** The form's schema (an object whose properties are the section's fields). */
  readonly schema: JsonSchema;
}

/** `["number", "null"]` becomes `number`, and `null` leaves enums: a patch's "keep" is not a value. */
export function denull(schema: JsonSchema): JsonSchema {
  const out: JsonSchema = { ...schema };
  if (Array.isArray(out.type)) {
    const types = out.type.filter((t: string) => t !== "null");
    out.type = types.length === 1 ? types[0] : types;
  }
  if (out.enum) out.enum = out.enum.filter((v) => v !== null);
  if (out.properties) {
    out.properties = Object.fromEntries(
      Object.entries(out.properties).map(([k, v]) => [k, denull(v)]),
    );
  }
  if (out.anyOf) out.anyOf = out.anyOf.filter((s) => s.type !== "null").map(denull);
  return out;
}

const hintOf = (schema: JsonSchema): UiHints => schema["x-ui"] ?? {};

/** The section is the group: inside its own form a property keeps no group of its own. */
function withoutGroup(schema: JsonSchema): JsonSchema {
  const { group: _group, ...hint } = hintOf(schema);
  return { ...schema, "x-ui": hint };
}

const isObject = (schema: JsonSchema): boolean =>
  schema.type === "object" || (Array.isArray(schema.type) && schema.type.includes("object"));

/** The `props` schema inside the input schema of `item.set_props`. */
export function propsSchema(input: JsonSchema): JsonSchema | null {
  const props = input.properties?.props;
  return props?.properties === undefined ? null : props;
}

/** The Video sections the schema defines, in the schema's `order`. */
export function videoSections(input: JsonSchema): VideoSection[] {
  return sectionsOf(input, (group) => !OTHER_SECTIONS.has(group));
}

/** The Audio section the schema defines (volume and pan: the properties of group `audio`). */
export function audioSection(input: JsonSchema): VideoSection | null {
  return sectionsOf(input, (group) => group === "audio")[0] ?? null;
}

/** The sections of the properties whose group `pick` accepts. */
function sectionsOf(input: JsonSchema, pick: (group: string) => boolean): VideoSection[] {
  const props = propsSchema(input);
  if (props === null || props.properties === undefined) return [];
  const byGroup = new Map<
    string,
    { order: number; entries: [string, JsonSchema][]; position: number }
  >();
  let position = 0;
  for (const [key, schema] of Object.entries(props.properties)) {
    const hint = hintOf(schema);
    const group = hint.group ?? OTHER_VIDEO_GROUP;
    position += 1;
    if (!pick(group) || hint.widget === "hidden") continue;
    const slot = byGroup.get(group) ?? { order: hint.order ?? 1000, entries: [], position };
    slot.order = Math.min(slot.order, hint.order ?? 1000);
    slot.entries.push([key, denull(schema)]);
    byGroup.set(group, slot);
  }
  return [...byGroup.entries()]
    .sort(([, a], [, b]) => a.order - b.order || a.position - b.position)
    .map(([id, slot]): VideoSection => {
      const resetGroup = RESET_GROUPS.has(id) ? (id as ResetGroup) : undefined;
      const [only] = slot.entries;
      if (slot.entries.length === 1 && only !== undefined && isObject(only[1])) {
        return { id, resetGroup, prefix: only[0], schema: only[1] };
      }
      return {
        id,
        resetGroup,
        prefix: "",
        schema: {
          type: "object",
          properties: Object.fromEntries(slot.entries.map(([k, v]) => [k, withoutGroup(v)])),
        },
      };
    });
}

/** What a property is reset to (`PropId::spec().default`, the engine's registry), by full path. */
export const PROP_DEFAULTS: Readonly<Record<string, unknown>> = {
  opacity: 1,
  blend_mode: "normal",
  "transform.position": { x: 0, y: 0 },
  "transform.scale": { x: 1, y: 1 },
  "transform.rotation": 0,
  "transform.anchor": { x: 0.5, y: 0.5 },
  "crop.left": 0,
  "crop.top": 0,
  "crop.right": 0,
  "crop.bottom": 0,
  "flip.horizontal": false,
  "flip.vertical": false,
  audio_gain: 0,
  audio_pan: 0,
};

/** The animatable properties (`PropId`), which are the dot paths into `props`. */
export const ANIMATABLE: ReadonlySet<string> = new Set<PropId>([
  "transform.position",
  "transform.scale",
  "transform.rotation",
  "transform.anchor",
  "opacity",
  "crop.left",
  "crop.top",
  "crop.right",
  "crop.bottom",
  "flip.horizontal",
  "flip.vertical",
  "audio_gain",
  "audio_pan",
]);

export const fullPath = (section: VideoSection, local: string): string =>
  section.prefix === "" ? local : local === "" ? section.prefix : `${section.prefix}.${local}`;

/** The section's defaults by form path. */
export function sectionDefaults(section: VideoSection): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [path, value] of Object.entries(PROP_DEFAULTS)) {
    if (section.prefix === "") {
      if (!path.includes(".") && section.schema.properties?.[path] !== undefined) out[path] = value;
    } else if (path.startsWith(`${section.prefix}.`)) {
      out[path.slice(section.prefix.length + 1)] = value;
    }
  }
  return out;
}

/** The properties of a section's fields that can carry keyframes, as form paths. */
export function keyframeablePaths(section: VideoSection): string[] {
  return Object.keys(sectionDefaults(section)).filter((p) => ANIMATABLE.has(fullPath(section, p)));
}

// ---- values -----------------------------------------------------------------------------

const isKeyed = (v: unknown): v is { keyframes: { value: unknown }[] } =>
  typeof v === "object" && v !== null && "keyframes" in v && Array.isArray(v.keyframes);

/**
 * An item's visual properties as plain values: an animated property shows `evaluated` (its value
 * at the playhead, from `keyframe.eval`) and, until that arrives, its first key's value.
 */
export function visualValue(
  item: Item,
  evaluated: Readonly<Record<string, unknown>> = {},
): Record<string, unknown> {
  const walk = (value: unknown, path: string): unknown => {
    if (isKeyed(value)) return path in evaluated ? evaluated[path] : value.keyframes[0]?.value;
    if (typeof value === "object" && value !== null && !Array.isArray(value)) {
      return Object.fromEntries(
        Object.entries(value).map(([k, v]) => [k, walk(v, path === "" ? k : `${path}.${k}`)]),
      );
    }
    return value;
  };
  return walk(item.visual, "") as Record<string, unknown>;
}

/** The audio properties as plain values (`audio_gain`, `audio_pan`), animated ones evaluated. */
export function audioValue(
  item: Item,
  evaluated: Readonly<Record<string, unknown>> = {},
): Record<string, unknown> {
  const one = (path: string, value: unknown): unknown =>
    isKeyed(value) ? (path in evaluated ? evaluated[path] : value.keyframes[0]?.value) : value;
  return {
    audio_gain: one("audio_gain", item.audio.gain),
    audio_pan: one("audio_pan", item.audio.pan),
  };
}

/** The value of a property by its `PropId` path, as stored (keyed or static). */
function stored(item: Item, path: string): unknown {
  if (path === "audio_gain") return item.audio.gain;
  if (path === "audio_pan") return item.audio.pan;
  return getPath(item.visual, path);
}

/**
 * The animated properties of an item (dot paths), static ones left out: the visual ones, or the
 * audio ones (`audio_gain`, `audio_pan`) with `which` `audio`.
 */
export function animatedPaths(item: Item, which: "visual" | "audio" = "visual"): string[] {
  if (which === "audio") {
    return ["audio_gain", "audio_pan"].filter((path) => isKeyed(stored(item, path)));
  }
  const out: string[] = [];
  const walk = (value: unknown, path: string): void => {
    if (isKeyed(value)) out.push(path);
    else if (typeof value === "object" && value !== null && !Array.isArray(value)) {
      for (const [k, v] of Object.entries(value)) walk(v, path === "" ? k : `${path}.${k}`);
    }
  };
  walk(item.visual, "");
  return out;
}

/** The key times (item time, flicks) of an animated property, or `null` when it is static. */
export function keyTimes(item: Item, path: string): readonly number[] | null {
  const value = stored(item, path);
  return isKeyed(value) ? (value.keyframes as unknown as { t: number }[]).map((k) => k.t) : null;
}

// ---- edits ------------------------------------------------------------------------------

/** `props` for `item.set_props` with only `path` set: every other key `null` ("keep"). */
export function propsPatch(props: JsonSchema | null, path: string, value: unknown): ItemProps {
  const fill = (schema: JsonSchema | undefined, patch: unknown): unknown => {
    if (schema?.properties === undefined) return patch;
    const given = (patch ?? {}) as Record<string, unknown>;
    return Object.fromEntries(
      Object.entries(schema.properties).map(([k, sub]) => [
        k,
        k in given ? fill(sub, given[k]) : null,
      ]),
    );
  };
  return fill(props ?? undefined, setPath({}, path, value)) as ItemProps;
}

// ---- keyframes --------------------------------------------------------------------------

export interface KeyFacts {
  readonly state: KeyframeState;
  /** Every selected clip covers the playhead: a key can be added. */
  readonly canAdd: boolean;
  /** The nearest key before and after the playhead, in sequence time. */
  readonly previous: number | null;
  readonly next: number | null;
}

/** What the diamond of one property shows across the selected clips, at the playhead. */
export function keyFacts(
  clips: readonly { readonly item: Item }[],
  path: string,
  playhead: number,
): KeyFacts {
  let animated = false;
  let onAll = clips.length > 0;
  let previous: number | null = null;
  let next: number | null = null;
  for (const { item } of clips) {
    const times = keyTimes(item, path);
    if (times === null) {
      onAll = false;
      continue;
    }
    animated = true;
    let here = false;
    for (const t of times) {
      const at = item.start + t;
      if (at === playhead) here = true;
      else if (at < playhead) previous = previous === null ? at : Math.max(previous, at);
      else next = next === null ? at : Math.min(next, at);
    }
    if (!here) onAll = false;
  }
  const canAdd = clips.every(
    (c) => playhead >= c.item.start && playhead < c.item.start + c.item.duration,
  );
  return {
    state: onAll ? "on" : animated ? "elsewhere" : "off",
    canAdd,
    previous,
    next,
  };
}
