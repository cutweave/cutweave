// SPDX-License-Identifier: GPL-3.0-or-later
// Replace clip (WP-2A.11): which clip and which bin card the person last picked, and which media
// can replace a clip. The editor keeps one selection list, so choosing a bin card lets go of the
// clip (and the other way round); a menu that says "replace this clip with the selected card"
// therefore reads the last single pick of each kind. It only remembers; the swap is the engine's
// `clip.replace` (it checks the same rules again and its answer wins).
import type { MediaAsset } from "@cutweave/protocol";
import { createStore } from "zustand/vanilla";
import { i18n } from "../i18n/index.ts";
import { clipFact, mediaAsset } from "../menus/surfaces/facts.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";

interface ReplacePick {
  /** The last clip picked alone on the timeline. */
  readonly clip: string | null;
  /** The last bin card picked alone. */
  readonly media: string | null;
}

export const replacePickStore = createStore<ReplacePick>()(() => ({ clip: null, media: null }));

/** Starts remembering picks from the view selection; returns the stop function. */
export function trackReplacePicks(): () => void {
  return viewStore.subscribe((state, previous) => {
    if (state.selection === previous.selection) return;
    const only = state.selection.length === 1 ? state.selection[0] : undefined;
    if (only?.kind === "item") replacePickStore.setState({ clip: only.id });
    else if (only?.kind === "media") replacePickStore.setState({ media: only.id });
  });
}

trackReplacePicks();

const isPicture = (kind: MediaAsset["kind"]): boolean => kind === "video" || kind === "image";

const hasSound = (asset: MediaAsset): boolean =>
  asset.kind === "audio" || (asset.kind === "video" && asset.probe.audio !== null);

/**
 * Whether `media` may replace what `clip` plays: pictures replace pictures, sound replaces
 * sound, a clip on an audio track needs media with sound, and neither compound clips nor freeze
 * frames are swapped. Mirrors the engine's rules (`clip.replace`).
 */
export function canReplaceWith(clip: string | null, media: string | null): boolean {
  if (clip === null || media === null) return false;
  const fact = clipFact(clip);
  const next = mediaAsset(media);
  const old = fact === null ? null : mediaAsset(fact.clip.media);
  if (fact === null || next === null || old === null) return false;
  if ((fact.clip.freeze ?? null) !== null || fact.clip.media === next.id) return false;
  if (old.kind === "sequence_ref" || next.kind === "sequence_ref") return false;
  if (isPicture(old.kind) !== isPicture(next.kind)) return false;
  return fact.track.kind !== "audio" || hasSound(next);
}

/** The clip the last single pick named, if it is still on the timeline. */
export function pickedClip(): string | null {
  const id = replacePickStore.getState().clip;
  return id !== null && clipFact(id) !== null ? id : null;
}

/** The bin card the last single pick named, if it is still in the project. */
export function pickedMedia(): string | null {
  const id = replacePickStore.getState().media;
  return id !== null && mediaAsset(id) !== null ? id : null;
}

/** What `clip.replace` answers that the person should hear about. */
interface ReplaceResult {
  readonly shortened?: boolean;
  readonly speed_reset?: boolean;
  readonly muted?: readonly string[];
}

export interface ReplaceArgs {
  readonly item: string;
  readonly media: string;
  readonly mode?: "keep_duration" | "keep_source_in" | "fit_to_source";
}

/** One `clip.replace`, then a notice for what the swap had to change on its own. */
export async function replaceClip(
  call: (name: string, args?: unknown) => Promise<unknown>,
  args: ReplaceArgs,
): Promise<void> {
  const result = (await call("clip.replace", args)) as ReplaceResult | undefined;
  const notes = [
    result?.shortened === true ? "contextMenu.dialog.replaceShortened" : null,
    result?.speed_reset === true ? "contextMenu.dialog.replaceSpeedReset" : null,
    (result?.muted?.length ?? 0) > 0 ? "contextMenu.dialog.replaceMuted" : null,
  ] as const;
  for (const key of notes) {
    if (key !== null) toastStore.getState().push({ tone: "info", title: i18n.t(key) });
  }
}
