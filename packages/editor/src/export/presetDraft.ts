// SPDX-License-Identifier: GPL-3.0-or-later
// "Save as preset" of the export dialog (WP-2G.04b): the preset that writes what the dialog
// shows now. It is the chosen preset with the person's changes folded in: a typed size or frame
// rate becomes the preset's own, a quality its CRF, the hardware switch `hw_preferred`, and the
// encoder options its `advanced` block (merged as the engine merges a request's over a
// preset's). The engine keeps it (`export.save_preset`, with the other presets of
// `export.list_presets`), so the CLI and agents see the same preset. Pure functions, tested alone.

import type {
  AdvancedExportOptions,
  ExportPresetEntry,
  ExportPresetSpec,
  ExportQuality,
  FrameRate,
} from "@cutweave/protocol";
import {
  presetQuality,
  pruneOptions,
  QUALITY_CRF,
  type SizeDraft,
  sizeProblem,
} from "./exportModel.ts";
import { parseFps } from "./exportSettings.ts";

/** The engine's limits on a preset's name and id (`MAX_PRESET_NAME_CHARS`, `MAX_PRESET_ID_CHARS`). */
export const MAX_PRESET_NAME = 80;
export const MAX_PRESET_ID = 64;

/** The id a name with no letter or digit the engine takes gets. */
const FALLBACK_ID = "my-preset";

/** Why a typed name cannot be a preset's: nothing typed, or longer than the engine keeps. */
export function nameProblem(name: string): "empty" | "long" | null {
  const trimmed = name.trim();
  if (trimmed === "") return "empty";
  return [...trimmed].length > MAX_PRESET_NAME ? "long" : null;
}

/** A name as a preset id: lowercase letters, digits and `-`, at most 64 characters. */
export function presetSlug(name: string): string {
  const slug = name
    .normalize("NFKD")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, MAX_PRESET_ID)
    .replace(/-+$/g, "");
  return slug === "" ? FALLBACK_ID : slug;
}

const sameName = (a: string, b: string): boolean =>
  a.trim().toLocaleLowerCase() === b.trim().toLocaleLowerCase();

/**
 * The id a preset saved under `name` gets. A preset of yours with that name is replaced (its id
 * is kept); otherwise the name's slug, with a number when another preset has it already (a
 * built-in id cannot be replaced).
 */
export function presetIdFor(
  name: string,
  entries: readonly ExportPresetEntry[],
): { readonly id: string; readonly replaces: ExportPresetSpec | null } {
  const mine = entries.find((e) => e.source === "user" && sameName(e.preset.name, name));
  if (mine !== undefined) return { id: mine.preset.id, replaces: mine.preset };
  const taken = new Set(entries.map((e) => e.preset.id));
  const base = presetSlug(name);
  if (!taken.has(base)) return { id: base, replaces: null };
  for (let n = 2; ; n++) {
    const suffix = `-${n}`;
    const id = `${base.slice(0, MAX_PRESET_ID - suffix.length).replace(/-+$/g, "")}${suffix}`;
    if (!taken.has(id)) return { id, replaces: null };
  }
}

type Advanced = NonNullable<AdvancedExportOptions>;

/**
 * `preset` with every setting `request` gives on top: rate control and GOP as a block, the rest
 * field by field (the engine's `advanced::merge`). `null` when neither sets anything.
 */
export function mergeAdvanced(
  preset: AdvancedExportOptions | null | undefined,
  request: Record<string, unknown> | null | undefined,
): AdvancedExportOptions | null {
  const p = (pruneOptions(preset) ?? {}) as Record<string, Record<string, unknown>>;
  const r = (pruneOptions(request) ?? {}) as Record<string, Record<string, unknown>>;
  const out: Record<string, Record<string, unknown>> = {};
  for (const section of ["video", "color", "audio"] as const) {
    const merged = { ...(p[section] ?? {}), ...(r[section] ?? {}) };
    if (Object.keys(merged).length > 0) out[section] = merged;
  }
  return Object.keys(out).length === 0 ? null : (out as Advanced);
}

/** What the dialog shows when Save as preset is pressed. */
export interface PresetDraft {
  /** The preset the dialog is on. */
  readonly base: ExportPresetSpec;
  readonly id: string;
  readonly name: string;
  /** One line for the picker ("From YouTube 1080p"). */
  readonly summary: string;
  /** Size and rate as typed in Advanced (`null`: the preset's own rule). */
  readonly size: SizeDraft;
  /** What the base preset writes for this sequence: fills the side that was not typed. */
  readonly resolved: { readonly width: number; readonly height: number };
  /** The quality chosen; `null` for a preset that sets a bitrate or a profile. */
  readonly quality: ExportQuality | null;
  /** The hardware switch. */
  readonly hardware: boolean;
  /** The encoder options of the Advanced form. */
  readonly options: Record<string, unknown>;
}

const whole = (text: string | null): number | null =>
  text !== null && /^\d+$/.test(text.trim()) ? Number(text.trim()) : null;

/**
 * The preset that writes what the dialog shows. Platform data (the platform, its safe area) is
 * left out: it belongs to the built-in presets, and the engine refuses it on a preset of yours.
 * The platform's limits stay, so the warnings do.
 */
export function presetFromDraft(draft: PresetDraft): ExportPresetSpec {
  const { base, size } = draft;
  const sound = sizeProblem(size) === null;
  const width = sound ? whole(size.width) : null;
  const height = sound ? whole(size.height) : null;
  const fps: FrameRate | null = sound && size.fps !== null ? parseFps(size.fps) : null;
  const video =
    base.video === null || base.video === undefined
      ? base.video
      : {
          ...base.video,
          rate_control:
            draft.quality !== null && draft.quality !== presetQuality(base)
              ? ({ mode: "crf", crf: QUALITY_CRF[draft.quality] } as const)
              : base.video.rate_control,
          hw_preferred: draft.hardware,
        };
  const advanced = mergeAdvanced(base.advanced, draft.options);
  const {
    platform: _platform,
    safe_area: _safeArea,
    advanced: _advanced,
    summary: _summary,
    ...kept
  } = base;
  return {
    ...kept,
    schema_version: 1,
    id: draft.id,
    name: draft.name.trim(),
    ...(draft.summary === "" ? {} : { summary: draft.summary }),
    ...(video === undefined ? {} : { video }),
    size:
      width !== null || height !== null
        ? {
            kind: "fixed",
            width: width ?? draft.resolved.width,
            height: height ?? draft.resolved.height,
          }
        : base.size,
    fps: fps !== null ? { kind: "fixed", rate: fps } : base.fps,
    ...(advanced === null ? {} : { advanced }),
  };
}
