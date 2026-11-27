// SPDX-License-Identifier: GPL-3.0-or-later
// "Import as slideshow…" and "Add frame to media bin" as the running app carries them out
// (WP-2F.05d): the menus' rows and the command palette's rows for
// `media.import_images_as_slideshow` and `media.import_frame` both come here, so there is one
// behaviour. The calls themselves are in `slideshow.ts`.
import type { MediaId } from "@cutweave/protocol";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { menuDialogStore } from "../menus/dialogs.ts";
import { playbackStore } from "../state/playbackStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { mediaFilesBridge, pickedPaths } from "./bridge.ts";
import { addFrameToBin, MIN_SLIDESHOW_PICTURES } from "./slideshow.ts";

/**
 * The native picker, then the slideshow dialog for the pictures chosen. The dialog sends the
 * one `media.import_images_as_slideshow`. Fewer than two pictures is not a slideshow: a notice
 * says so.
 */
export async function pickSlideshow(): Promise<void> {
  const pick = mediaFilesBridge().pickMediaFiles;
  if (pick === undefined) {
    toastStore.getState().push({ tone: "info", title: i18n.t("mediaBin.import.desktopOnly") });
    return;
  }
  const files = pickedPaths(await pick().catch(() => null));
  if (files.length === 0) return;
  if (files.length < MIN_SLIDESHOW_PICTURES) {
    toastStore.getState().push({
      tone: "info",
      title: i18n.t("mediaBin.slideshow.tooFewTitle"),
      message: i18n.t("mediaBin.slideshow.tooFewBody"),
    });
    return;
  }
  menuDialogStore.getState().open({ kind: "slideshow", files });
}

/**
 * The frame on screen as a picture in the bin: the Source view's while a source plays, else the
 * Preview's at the playhead. One `media.import_frame`.
 */
export async function addShownFrameToBin(): Promise<MediaId | null> {
  const playback = playbackStore.getState();
  return addFrameToBin(
    { engine: engineStore, toasts: toastStore, t: translator(i18n.t.bind(i18n)) },
    playback.source === null
      ? { kind: "sequence", at: viewStore.getState().playhead }
      : { kind: "media", media: playback.source.media, at: playback.sourcePlayhead },
  );
}
