// SPDX-License-Identifier: GPL-3.0-or-later
// A slideshow from pictures, and the Preview's frame as a picture in the bin (WP-2F.05d,
// ADR-0035). Both are one catalog command each, the same call the CLI and MCP make:
// `media.import_images_as_slideshow` (the stills and their clips, one undo step) and
// `media.import_frame` (the frame on screen as a PNG in the project's `generated/frames/`
// folder). This file picks the arguments from what the editor shows and says what happened.
import {
  type ImportFailure,
  type MediaId,
  RpcError,
  type SequenceId,
  type SlideshowOrder,
} from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";
import { describeImportFailure, MAX_IMPORT_PATHS, validateImportPaths } from "./import.ts";

/** A slideshow is two pictures or more; one picture is an ordinary import. */
export const MIN_SLIDESHOW_PICTURES = 2;
/** The longest a picture may stay, in seconds (the bound of the still duration, ADR-0035). */
export const MAX_SECONDS_PER_PICTURE = 3600;
/** What the dialog proposes when the project does not say (the engine's own default). */
export const DEFAULT_SECONDS_PER_PICTURE = 5;

export const SLIDESHOW_ORDERS: readonly SlideshowOrder[] = ["given", "name", "name_numeric"];

export interface SlideshowDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
}

const baseName = (path: string): string => path.split(/[\\/]/).pop() || path;

/** The seconds a person typed ("2", "2.5", "0,5"), or `null` when it is not a usable length. */
export function parseSeconds(text: string): number | null {
  const trimmed = text.trim().replace(",", ".");
  if (!/^\d+(\.\d+)?$/.test(trimmed)) return null;
  const seconds = Number(trimmed);
  return Number.isFinite(seconds) && seconds > 0 && seconds <= MAX_SECONDS_PER_PICTURE
    ? seconds
    : null;
}

/** The project's still duration in seconds, as the dialog's first proposal. */
export function proposedSeconds(stillDuration: number | null | undefined): string {
  if (typeof stillDuration !== "number" || stillDuration <= 0) {
    return String(DEFAULT_SECONDS_PER_PICTURE);
  }
  return String(Math.round((stillDuration / FLICKS_PER_SECOND) * 1000) / 1000);
}

export interface SlideshowRequest {
  readonly files: readonly unknown[];
  readonly seconds: number;
  readonly order: SlideshowOrder;
  /** The sequence the editor shows; left out, the engine takes the project's only one. */
  readonly sequence: SequenceId | null;
}

/**
 * One `media.import_images_as_slideshow` for the pictures, then toasts: one per file that was
 * left out, and how many pictures went onto the timeline. Resolves with the number of clips,
 * or `null` when nothing was sent or the call failed (which shows its own toast).
 */
export async function importSlideshow(
  deps: SlideshowDeps,
  request: SlideshowRequest,
): Promise<number | null> {
  const { toasts, t } = deps;
  const files = validateImportPaths(request.files).slice(0, MAX_IMPORT_PATHS);
  const rpc = deps.engine.getState().rpc;
  if (rpc === null || files.length === 0) return null;
  let failed: readonly ImportFailure[];
  let clips: number;
  try {
    const out = await rpc.call("media.import_images_as_slideshow", {
      files,
      per_image: Math.round(request.seconds * FLICKS_PER_SECOND),
      order: request.order,
      ...(request.sequence === null ? {} : { sequence: request.sequence }),
    });
    failed = out.failed;
    clips = out.clips.length;
  } catch (error) {
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.slideshow.failed"),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return null;
  }
  for (const failure of failed) {
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.import.failedTitle", { name: baseName(failure.path) }),
      message:
        failure.code === "unsupported"
          ? t("mediaBin.slideshow.notPicture")
          : describeImportFailure(failure, t),
    });
  }
  toasts.getState().push({
    tone: "success",
    title: t("mediaBin.slideshow.doneTitle"),
    message: t("mediaBin.slideshow.doneBody", { count: String(clips) }),
  });
  return clips;
}

/** What "Add frame to media bin" takes the frame of. */
export type FrameSource =
  | { readonly kind: "sequence"; readonly at: number }
  | { readonly kind: "media"; readonly media: MediaId; readonly at: number };

/**
 * One `media.import_frame` for the frame on screen, then a toast that names the new picture
 * (or says it was in the bin already). Resolves with the asset, or `null` when the call was not
 * sent or failed: an untitled project is refused by the engine with "save the project first",
 * which the toast shows.
 */
export async function addFrameToBin(
  deps: SlideshowDeps,
  source: FrameSource,
): Promise<MediaId | null> {
  const { toasts, t } = deps;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("media.import_frame", {
      source: source.kind === "sequence" ? "sequence" : { media: source.media },
      at: Math.max(0, Math.round(source.at)),
    });
    toasts.getState().push({
      tone: out.duplicate ? "info" : "success",
      title: t(out.duplicate ? "mediaBin.frame.duplicateTitle" : "mediaBin.frame.doneTitle"),
      message: t(out.duplicate ? "mediaBin.frame.duplicateBody" : "mediaBin.frame.doneBody", {
        name: out.name,
      }),
    });
    return out.media_id;
  } catch (error) {
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.frame.failed"),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return null;
  }
}
