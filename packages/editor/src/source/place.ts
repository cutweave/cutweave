// SPDX-License-Identifier: GPL-3.0-or-later
// Insert and Overwrite from the Source view (WP-2A.14, timeline spec §4.18): the marked range of
// the source (or all of it) goes to the timeline at the timeline's in point, else its playhead,
// on the selected (or first fitting) track. One `timeline.insert` (mode `insert`: what is there
// is split and moves later) or `timeline.overwrite` (what is there is cut out): one undo step.
import type {
  ItemId,
  MediaId,
  Project,
  RpcClient,
  SelectionRef,
  SequenceId,
  SourceMarks,
  TrackId,
} from "@cutweave/protocol";
import { type InsertDeps, pickTrack } from "../media/insert.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { selectActiveSequenceId, selectInOut } from "../state/selectors.ts";
import { describeRefusal, FAILED_TITLE } from "../timeline/errors.ts";
import { toRpcError } from "../timeline/gesture.ts";
import { callWithNewTrackHeight } from "../timeline/newTracks.ts";
import { newTrackHeight } from "../timeline/prefs.ts";
import { threePoint } from "./threePoint.ts";

export type PlaceMode = "insert" | "overwrite";

/** What a still without marks lasts, flicks (INV-25). */
const STILL_LENGTH = 5 * 705_600_000;

/**
 * The command names as template literals on purpose: the i18n catalog test reads a double-quoted
 * "timeline.…" string as a translation key (the catalog has a `timeline` root).
 */
const COMMAND: Readonly<Record<PlaceMode, string>> = {
  insert: `timeline.insert`,
  overwrite: `timeline.overwrite`,
};

export interface PlaceSource {
  readonly id: MediaId;
  /** What it goes on: a video (with its sound) on a video track, sound on an audio track. */
  readonly kind: "video" | "audio";
  /** The media's length, flicks. */
  readonly duration: number;
}

export interface PlaceOptions {
  /** `false`: the picture without its sound. */
  readonly link?: boolean;
}

export interface PlaceArgs {
  readonly sequence: SequenceId;
  readonly track: TrackId;
  readonly media: MediaId;
  readonly at: number;
  readonly source_in: number;
  readonly duration: number;
  readonly mode?: "insert";
  readonly link?: false;
}

export type PlacePlan =
  | { readonly ok: true; readonly args: PlaceArgs; readonly shortened: boolean }
  | { readonly ok: false; readonly reason: "no_track" | "invalid" };

/**
 * The arguments of the placement: spec §4.18 over the source's marks, the timeline's in and out
 * and the playhead. The engine snaps `at` and the length to the sequence's frames.
 */
export function planPlacement(input: {
  readonly project: Project | null;
  readonly playhead: number;
  readonly track: TrackId | null;
  readonly source: PlaceSource;
  readonly marks: SourceMarks | undefined;
  readonly mode: PlaceMode;
  readonly link?: boolean | undefined;
}): PlacePlan {
  const sequence = selectActiveSequenceId(input.project);
  if (sequence === null || input.track === null) return { ok: false, reason: "no_track" };
  const point = threePoint({
    playhead: input.playhead,
    timeline: selectInOut(input.project),
    sourceIn: input.marks?.in ?? null,
    sourceOut: input.marks?.out ?? null,
    mediaLength: input.source.duration,
    stillLength: STILL_LENGTH,
  });
  if (!point.ok) return { ok: false, reason: "invalid" };
  return {
    ok: true,
    shortened: point.shortened,
    args: {
      sequence,
      track: input.track,
      media: input.source.id,
      at: point.at,
      source_in: point.sourceIn,
      duration: point.duration,
      ...(input.mode === "insert" && { mode: "insert" as const }),
      ...(input.link === false && { link: false as const }),
    },
  };
}

/**
 * Places the source's marked range on the timeline and selects the new clip. A refusal shows a
 * toast and resolves to `false`; nothing throws.
 */
export async function placeSource(
  deps: InsertDeps,
  source: PlaceSource,
  mode: PlaceMode,
  options: PlaceOptions = {},
): Promise<boolean> {
  const rpc: RpcClient | null = deps.engine.getState().rpc;
  const view = deps.view.getState();
  const plan = planPlacement({
    project: deps.doc.getState().project,
    playhead: view.playhead,
    track: pickTrack(deps, source.kind),
    source,
    marks: view.sourceMarks[source.id],
    mode,
    link: options.link === false || !preferences.get("editing.link_av") ? false : undefined,
  });
  if (rpc === null) return false;
  if (!plan.ok) {
    if (plan.reason === "invalid") {
      deps.toasts.getState().push({
        tone: "warning",
        title: deps.t(FAILED_TITLE.insert),
        message: deps.t("preview.source.nothingToPlace"),
      });
    }
    return false;
  }
  try {
    const height = newTrackHeight(
      preferences.get("timeline.track_height"),
      globalThis.document?.documentElement ?? null,
    );
    const out = (await callWithNewTrackHeight(rpc, COMMAND[mode], plan.args, height)) as {
      clips: string[];
    };
    const first = out.clips[0];
    if (first !== undefined) {
      deps.view.getState().setSelection([{ kind: "item", id: first as ItemId } as SelectionRef]);
    }
    if (plan.shortened) {
      deps.toasts.getState().push({
        tone: "info",
        title: deps.t("preview.source.shortenedTitle"),
        message: deps.t("preview.source.shortened"),
      });
    }
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "warning",
      title: deps.t(FAILED_TITLE.insert),
      message: describeRefusal(toRpcError(error), deps.t),
    });
    return false;
  }
}
