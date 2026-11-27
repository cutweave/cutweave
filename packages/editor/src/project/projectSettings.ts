// SPDX-License-Identifier: GPL-3.0-or-later
// File > Project settings… (WP-2H.08): the model behind the dialog. The form is drawn from the
// schema of `project.set_settings`'s `patch` (generated from the engine's `ProjectSettings`), so
// a key a later work package adds shows up as a control with no change here. The keys this file
// knows get a nicer presentation (seconds for the still duration, a list of frame rates, the
// translated names); every other key is drawn as the schema describes it.
import type { ProjectSettings, ProxyPolicy } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import type { JsonSchema, UiHints } from "@cutweave/ui";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { commandCatalog } from "../palette/catalog.ts";
import { asRpcError } from "../session/errors.ts";
import type { Translate } from "../timeline/errors.ts";
import { formatPreference } from "./newSequenceFormat.ts";

/** The engine command the dialog sends. */
export const SET_SETTINGS = "project.set_settings";

/** Five seconds: the still duration of a project that stores none (ADR-0035). */
const DEFAULT_STILL_SECONDS = 5;
const MAX_STILL_SECONDS = 3600;
const MAX_CANVAS_SIDE = 16384;

/** The frame rates offered for new sequences, as `num/den` (the Inspector offers the same). */
const RATES: readonly { num: number; den: number }[] = [
  { num: 24000, den: 1001 },
  { num: 24, den: 1 },
  { num: 25, den: 1 },
  { num: 30000, den: 1001 },
  { num: 30, den: 1 },
  { num: 50, den: 1 },
  { num: 60000, den: 1001 },
  { num: 60, den: 1 },
];
const SAMPLE_RATES: readonly number[] = [44100, 48000, 96000];

const rateKey = (r: { num: number; den: number }): string => `${r.num}/${r.den}`;
const rateFps = (r: { num: number; den: number }): string =>
  String(Math.round((r.num / r.den) * 1000) / 1000);

/** The schema of the settings a project has: the `patch` of `project.set_settings`. */
export function settingsSchema(): JsonSchema {
  const command = commandCatalog.getState().commands.find((c) => c.name === SET_SETTINGS);
  const patch = command?.inputSchema.properties?.patch;
  return (patch ?? { type: "object", properties: {} }) as JsonSchema;
}

/** Translated names of the sections the engine's hints name (`x-ui.group`); others show as written. */
const GROUPS: Readonly<Record<string, TKey>> = {
  "New sequences": "projectSettings.groups.newSequences",
  Media: "projectSettings.groups.media",
};

/** Translated labels of the keys this version knows, by dot path. */
const LABELS: Readonly<Record<string, TKey>> = {
  default_format: "projectSettings.fields.format",
  "default_format.width": "inspector.sequence.width",
  "default_format.height": "inspector.sequence.height",
  "default_format.frame_rate": "inspector.sequence.frameRate",
  "default_format.sample_rate": "projectSettings.fields.sampleRate",
  "default_format.color_space": "projectSettings.fields.colorSpace",
  "default_format.background": "inspector.sequence.background",
  still_duration: "settings.rows.stillDuration.label",
  proxy_policy: "projectSettings.fields.proxies",
};

const PROXY_LABELS: Readonly<Record<ProxyPolicy, TKey>> = {
  off: "settings.rows.proxyPolicy.off",
  auto: "settings.rows.proxyPolicy.auto",
  all: "settings.rows.proxyPolicy.all",
};

type Props = Record<string, JsonSchema>;

function withHint(schema: JsonSchema | undefined, hint: UiHints): JsonSchema {
  return { ...schema, "x-ui": { ...schema?.["x-ui"], ...hint } };
}

/** A `oneOf` of constants with the titles the form shows, keeping the hints of `from`. */
function choice(
  from: JsonSchema | undefined,
  options: readonly { value: string | number; title: string }[],
  hint: UiHints,
): JsonSchema {
  return {
    oneOf: options.map((o) => ({ const: o.value, title: o.title })),
    "x-ui": { ...from?.["x-ui"], ...hint },
  };
}

function constantsOf(schema: JsonSchema | undefined): string[] {
  const variants = schema?.oneOf ?? schema?.anyOf ?? [];
  const consts = variants.map((v) => v.const).filter((v): v is string => typeof v === "string");
  return consts.length > 0 ? consts : ((schema?.enum ?? []) as string[]);
}

export interface SettingsForm {
  /** The schema the form draws: the engine's, with the known keys presented for people. */
  readonly schema: JsonSchema;
  /** Labels and section names by dot path. */
  readonly hints: Readonly<Record<string, UiHints>>;
  /** What the form shows for the project's settings. */
  readonly value: Record<string, unknown>;
}

/**
 * The form over `settings`: `schema` is the engine's (see [`settingsSchema`]). Keys this file
 * does not know pass through untouched.
 */
export function settingsForm(
  schema: JsonSchema,
  settings: ProjectSettings,
  t: Translate,
): SettingsForm {
  const props: Props = { ...(schema.properties as Props | undefined) };
  const value: Record<string, unknown> = { ...settings };
  const hints: Record<string, UiHints> = {};

  if (props.still_duration !== undefined) {
    props.still_duration = {
      type: "number",
      minimum: 0.01,
      maximum: MAX_STILL_SECONDS,
      "x-ui": {
        ...props.still_duration["x-ui"],
        unit: t("projectSettings.seconds"),
        step: 1,
        precision: 2,
      },
    };
    const flicks = settings.still_duration;
    value.still_duration =
      typeof flicks === "number"
        ? Math.round((flicks / FLICKS_PER_SECOND) * 1000) / 1000
        : DEFAULT_STILL_SECONDS;
  }

  if (props.proxy_policy !== undefined) {
    const names = constantsOf(props.proxy_policy);
    props.proxy_policy = choice(
      props.proxy_policy,
      names.map((name) => ({
        value: name,
        title: name in PROXY_LABELS ? t(PROXY_LABELS[name as ProxyPolicy]) : name,
      })),
      { widget: "segmented" },
    );
    value.proxy_policy = settings.proxy_policy ?? "auto";
  }

  const format = props.default_format;
  if (format?.properties !== undefined) {
    const inner: Props = { ...(format.properties as Props) };
    const current = settings.default_format;
    if (inner.width !== undefined) {
      inner.width = withHint(
        { ...inner.width, minimum: 1, maximum: MAX_CANVAS_SIDE },
        { unit: "px", step: 1, order: 0 },
      );
    }
    if (inner.height !== undefined) {
      inner.height = withHint(
        { ...inner.height, minimum: 1, maximum: MAX_CANVAS_SIDE },
        { unit: "px", step: 1, order: 1 },
      );
    }
    if (inner.frame_rate !== undefined) {
      const rates = RATES.some((r) => rateKey(r) === rateKey(current.frame_rate))
        ? RATES
        : [...RATES, current.frame_rate];
      inner.frame_rate = choice(
        inner.frame_rate,
        rates.map((r) => ({
          value: rateKey(r),
          title: t("inspector.sequence.rateValue", { fps: rateFps(r) }),
        })),
        { widget: "select", order: 2 },
      );
    }
    if (inner.sample_rate !== undefined) {
      const rates = SAMPLE_RATES.includes(current.sample_rate)
        ? SAMPLE_RATES
        : [...SAMPLE_RATES, current.sample_rate];
      inner.sample_rate = choice(
        inner.sample_rate,
        rates.map((rate) => ({
          value: rate,
          title: t("projectSettings.hz", { rate: String(rate) }),
        })),
        { widget: "select", order: 4 },
      );
    }
    if (inner.color_space !== undefined) {
      inner.color_space = choice(
        inner.color_space,
        constantsOf(inner.color_space).map((name) => ({
          value: name,
          title: name === "rec709" ? t("projectSettings.options.rec709") : name,
        })),
        { widget: "select", order: 5 },
      );
    }
    if (inner.background !== undefined) {
      inner.background = withHint(inner.background, { widget: "color", order: 3 });
    }
    // The background fill (WP-2B.02) has no row here: it is set per sequence, in the Inspector.
    const { background_fill: _fill, ...shown } = inner;
    props.default_format = { ...format, properties: shown };
    value.default_format = { ...current, frame_rate: rateKey(current.frame_rate) };
  }

  for (const [path, key] of Object.entries(LABELS)) hints[path] = { label: t(key) };
  // Section names: the engine's hint names the section; the known ones are translated.
  for (const [key, prop] of Object.entries(props)) {
    const group = prop["x-ui"]?.group;
    const known = group === undefined ? undefined : GROUPS[group];
    if (known !== undefined) hints[key] = { ...hints[key], group: t(known) };
  }
  return { schema: { ...schema, properties: props }, hints, value };
}

function nest(path: string, value: unknown): Record<string, unknown> {
  const keys = path.split(".");
  let out: unknown = value;
  for (let i = keys.length - 1; i >= 0; i--) out = { [keys[i] as string]: out };
  return out as Record<string, unknown>;
}

/**
 * The patch `project.set_settings` takes for one edited field of the form (`path` is its dot
 * path), or `null` when the value cannot be one (a half-typed number).
 */
export function patchFor(path: string, value: unknown): Record<string, unknown> | null {
  if (path === "still_duration") {
    const seconds = Number(value);
    if (!Number.isFinite(seconds) || seconds <= 0) return null;
    return { still_duration: Math.round(seconds * FLICKS_PER_SECOND) };
  }
  if (path === "default_format.frame_rate") {
    const [num, den] = String(value).split("/").map(Number);
    if (!Number.isInteger(num) || !Number.isInteger(den)) return null;
    return nest(path, { num, den });
  }
  if (path === "default_format.width" || path === "default_format.height") {
    const side = Math.round(Number(value));
    if (!Number.isFinite(side) || side < 1) return null;
    return nest(path, side);
  }
  if (path === "default_format.sample_rate") return nest(path, Number(value));
  return nest(path, value);
}

/** `/patch/default_format/width` (where the engine refused a value) as the form's `default_format.width`. */
export function fieldOfPointer(pointer: string | undefined): string | null {
  if (pointer === undefined || !pointer.startsWith("/patch/")) return null;
  return pointer.slice("/patch/".length).split("/").join(".");
}

// ----- the dialog's state ----------------------------------------------------------------------

export interface ProjectSettingsState {
  readonly open: boolean;
  /** Why the engine refused the last change, by the form's dot path. */
  readonly errors: Readonly<Record<string, string>>;
  /** What "Use as defaults for new projects" last did, said under the form. */
  readonly notice: string | null;
  show(): void;
  close(): void;
  set(change: Partial<Pick<ProjectSettingsState, "errors" | "notice">>): void;
}

export const projectSettingsStore = createStore<ProjectSettingsState>()((set) => ({
  open: false,
  errors: {},
  notice: null,
  show: () => set({ open: true, errors: {}, notice: null }),
  close: () => set({ open: false }),
  set: (change) => set(change),
}));

/** Opens File > Project settings…. */
export function openProjectSettings(): void {
  projectSettingsStore.getState().show();
}

export interface SettingsDeps {
  readonly engine: StoreApi<EngineStoreState>;
}

/**
 * Sends one change: one `project.set_settings`, so one undo step. A refusal shows under its
 * field (or under the form when it names none) and resolves `false`.
 */
export async function applySetting(
  deps: SettingsDeps,
  path: string,
  value: unknown,
  refused: string,
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  const patch = patchFor(path, value);
  if (rpc === null || patch === null) return false;
  try {
    await rpc.call(SET_SETTINGS, { patch } as never);
    projectSettingsStore.getState().set({ errors: {}, notice: null });
    return true;
  } catch (error) {
    const rpcError = asRpcError(error);
    const field = fieldOfPointer(rpcError.path) ?? path;
    projectSettingsStore.getState().set({
      errors: { [field]: rpcError.message || refused, [path]: rpcError.message || refused },
    });
    return false;
  }
}

/** The app preferences "Use as defaults for new projects" writes, from the project's settings. */
export function defaultsOf(settings: ProjectSettings): {
  readonly stillSeconds: number;
  readonly proxyPolicy: ProxyPolicy;
  /** `editing.sequence_format`: the project's format of new sequences (#221); `""` if it has no spelling. */
  readonly sequenceFormat: string;
} {
  const flicks = settings.still_duration;
  const seconds = typeof flicks === "number" ? flicks / FLICKS_PER_SECOND : DEFAULT_STILL_SECONDS;
  return {
    // The preference counts whole seconds, from 1 to an hour.
    stillSeconds: Math.min(MAX_STILL_SECONDS, Math.max(1, Math.round(seconds))),
    proxyPolicy: settings.proxy_policy ?? "auto",
    // The format the dialog shows under "New sequences", not the format of any one sequence.
    sequenceFormat: formatPreference(settings.default_format),
  };
}
