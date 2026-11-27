// SPDX-License-Identifier: GPL-3.0-or-later
// What the Source view's buttons, keys and menu rows run (WP-2A.14), over the app's own stores:
// set and clear the marks, Insert and Overwrite the marked range, and match frame. The marks are
// view state (`view.set{source_marks}`); the placements are `timeline.insert` and
// `timeline.overwrite`, so the CLI and MCP make the same edit with the same call.
import type { MediaId } from "@cutweave/protocol";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { playbackStore } from "../state/playbackStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { transportController } from "../transport/useTransport.ts";
import { clearSourceMarks, type MarkPoint, markSource } from "./marks.ts";
import { matchFrameAt } from "./matchFrame.ts";
import { type PlaceMode, type PlaceOptions, type PlaceSource, placeSource } from "./place.ts";

const MARKS = { view: viewStore, playback: playbackStore };

/** The source on screen as a placement needs it, or `null` when the timeline is shown. */
export function shownSource(): PlaceSource | null {
  const source = playbackStore.getState().source;
  if (source === null) return null;
  const media = docStore.getState().project?.media[source.media];
  return {
    id: source.media as MediaId,
    kind: media?.kind === "audio" ? "audio" : "video",
    duration: source.duration,
  };
}

/** I and O in the Source view: the mark goes to the source's own playhead. */
export function setSourceMark(point: MarkPoint): boolean {
  return markSource(MARKS, point);
}

/** "Clear in and out" of the source on screen. */
export function clearMarks(): boolean {
  return clearSourceMarks(MARKS);
}

/** Whether the source on screen has a mark. */
export function sourceHasMarks(): boolean {
  const source = playbackStore.getState().source;
  return source !== null && viewStore.getState().sourceMarks[source.media as MediaId] !== undefined;
}

/**
 * Insert (`,`) or Overwrite (`.`): the marked range of the source on screen (all of it without
 * marks) goes to the timeline. One undo step.
 */
export function placeShownSource(mode: PlaceMode, options: PlaceOptions = {}): Promise<boolean> {
  const source = shownSource();
  if (source === null) return Promise.resolve(false);
  return placeSource(
    {
      doc: docStore,
      view: viewStore,
      engine: engineStore,
      toasts: toastStore,
      t: translator(i18n.t.bind(i18n)),
    },
    source,
    mode,
    options,
  );
}

/**
 * Match frame (F): the clip under the timeline's playhead opens in the Source view at the frame
 * the playhead shows, with the part of the media the clip plays as its marks. Resolves to
 * whether a clip was there.
 */
export async function matchFrame(): Promise<boolean> {
  const view = viewStore.getState();
  const selected = view.selection.filter((r) => r.kind === "item").map((r) => r.id as string);
  const match = matchFrameAt(docStore.getState().project, view.playhead, selected);
  if (match === null) {
    toastStore.getState().push({ tone: "info", title: i18n.t("timeline.matchFrame.none") });
    return false;
  }
  if (!(await transportController().showSource(match.media, match.at))) return false;
  viewStore.getState().setSourceMarks(match.media, { in: match.in, out: match.out });
  return true;
}
