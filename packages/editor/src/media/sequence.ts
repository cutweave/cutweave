// SPDX-License-Identifier: GPL-3.0-or-later
// Image sequences in the bin (WP-2F.05c, ADR-0035): a numbered run of pictures imported as one
// item that plays like video.
//
// - `runOfSelection` says whether the files a person picked are such a run (same folder, same
//   name but for the number at its end, a picture extension, two or more). The engine never
//   guesses: `media.import` keeps every picture a still, so the app asks (`SequenceOffer.tsx`).
// - `importSequence` sends the one `media.import_sequence` and says what happened with toasts.
// - `importPicked` is what the Import… picker and a drop of files from the OS run: the offer
//   when the files are a run, then the import the person chose.

import type { ImageSequenceFps } from "@cutweave/preferences";
import type { FolderId, FrameRate } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { createStore } from "zustand/vanilla";
import { type ImportDeps, type ImportOptions, type ImportSummary, importMedia } from "./import.ts";

/** The extensions that count as pictures of a run (the engine's list for folders). */
const PICTURE_EXTENSIONS = new Set([
  "png",
  "jpg",
  "jpeg",
  "tif",
  "tiff",
  "bmp",
  "webp",
  "exr",
  "dpx",
  "tga",
]);

/** A picked selection that is one numbered run. */
export interface PickedRun {
  /** The path of the lowest-numbered file picked: what `media.import_sequence` is given. */
  readonly path: string;
  /** The file names of the lowest and the highest number picked. */
  readonly first: string;
  readonly last: string;
  /** How many files were picked. */
  readonly count: number;
}

interface NumberedName {
  readonly dir: string;
  readonly prefix: string;
  readonly digits: string;
  readonly ext: string;
}

/** `/r/shot_0098.png` as its folder, `shot_`, `0098` and `.png`; `null` without a final number. */
function numbered(path: string): NumberedName | null {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\")) + 1;
  const name = path.slice(cut);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : "";
  const match = /^(.*?)(\d+)$/.exec(stem);
  if (match === null) return null;
  return { dir: path.slice(0, cut), prefix: match[1] ?? "", digits: match[2] ?? "", ext };
}

/**
 * The run `paths` are, or `null`: two or more pictures of one folder whose names differ only in
 * the number at their end, each number once.
 */
export function runOfSelection(paths: readonly string[]): PickedRun | null {
  if (paths.length < 2) return null;
  const parts: { path: string; name: NumberedName; number: number }[] = [];
  for (const path of paths) {
    const name = numbered(path);
    if (name === null || !PICTURE_EXTENSIONS.has(name.ext.slice(1).toLowerCase())) return null;
    const number = Number(name.digits);
    if (!Number.isSafeInteger(number)) return null;
    parts.push({ path, name, number });
  }
  const head = parts[0]?.name;
  if (head === undefined) return null;
  const same = parts.every(
    (p) => p.name.dir === head.dir && p.name.prefix === head.prefix && p.name.ext === head.ext,
  );
  if (!same || new Set(parts.map((p) => p.number)).size !== parts.length) return null;
  parts.sort((a, b) => a.number - b.number);
  const [first, last] = [parts[0], parts[parts.length - 1]];
  if (first === undefined || last === undefined) return null;
  const fileName = (p: (typeof parts)[number]) => p.path.slice(p.name.dir.length);
  return { path: first.path, first: fileName(first), last: fileName(last), count: parts.length };
}

const RATES: Readonly<Record<Exclude<ImageSequenceFps, "sequence">, FrameRate>> = {
  "23.976": { num: 24000, den: 1001 },
  "24": { num: 24, den: 1 },
  "25": { num: 25, den: 1 },
  "29.97": { num: 30000, den: 1001 },
  "30": { num: 30, den: 1 },
  "50": { num: 50, den: 1 },
  "59.94": { num: 60000, den: 1001 },
  "60": { num: 60, den: 1 },
};

/**
 * The `fps` of `media.import_sequence` for the setting `media.image_sequence_fps`: a rate, or
 * nothing for "the sequence's frame rate" (the engine's default).
 */
export function sequenceFps(setting: ImageSequenceFps): FrameRate | undefined {
  return setting === "sequence" ? undefined : RATES[setting];
}

/** `30`, `23.976`: a frame rate for a sentence. */
export function fpsText(rate: FrameRate): string {
  const fps = rate.num / rate.den;
  return Number.isInteger(fps) ? String(fps) : fps.toFixed(3).replace(/0+$/, "");
}

export interface SequenceImportOptions {
  /** The setting `media.image_sequence_fps`. */
  readonly fps: ImageSequenceFps;
  /**
   * Whether every picture of the run is first copied into the project's `media/` folder (the
   * setting `media.copy_on_import`). Only `true` is sent; the engine's default is off.
   */
  readonly copyToProject?: boolean;
}

/**
 * Imports the run `path` belongs to (or holds, for a folder) as one image sequence into `folder`
 * of the bin, with toasts: what was imported, and how many numbers have no picture. Resolves
 * with the new asset's id, or `null` when nothing was imported (the failure has its own toast).
 */
export async function importSequence(
  deps: ImportDeps,
  path: string,
  folder: FolderId | null = null,
  options: SequenceImportOptions = { fps: "sequence" },
): Promise<string | null> {
  const { toasts, t } = deps;
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  const progress = toasts.getState().push({ tone: "info", title: t("mediaBin.sequence.working") });
  const fps = sequenceFps(options.fps);
  try {
    const out = await rpc.call("media.import_sequence", {
      path,
      ...(fps === undefined ? {} : { fps }),
      ...(folder === null ? {} : { folder }),
      ...(options.copyToProject === true ? { copy_to_project: true } : {}),
    });
    toasts.getState().dismiss(progress);
    const run = out.image_sequence;
    const missing = run.missing.length;
    toasts.getState().push({
      tone: "success",
      title: t("mediaBin.sequence.doneTitle"),
      message: out.duplicate
        ? t("mediaBin.sequence.duplicate")
        : t("mediaBin.sequence.doneBody", {
            count: String(run.count - missing),
            fps: fpsText(run.fps),
          }),
    });
    if (missing > 0 && !out.duplicate) {
      toasts.getState().push({
        tone: "warning",
        title: t("mediaBin.sequence.gapsTitle"),
        message:
          missing === 1
            ? t("mediaBin.sequence.gapsOne")
            : t("mediaBin.sequence.gapsMany", { count: String(missing) }),
      });
    }
    return out.media_id;
  } catch (error) {
    toasts.getState().dismiss(progress);
    toasts.getState().push({
      tone: "danger",
      title: t("mediaBin.sequence.failedTitle"),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return null;
  }
}

// ---- the offer --------------------------------------------------------------------------------

/** What the person answers: one sequence, separate stills, or nothing. */
export type SequenceChoice = "sequence" | "separate" | "cancel";

interface SequenceOfferStore {
  /** The run being asked about; `null` when no question is open. */
  readonly offer: PickedRun | null;
  /** Opens the question; resolves with the answer (a second question cancels the first). */
  ask(run: PickedRun): Promise<SequenceChoice>;
  /** Closes the question with `choice`. */
  answer(choice: SequenceChoice): void;
}

let resolveOffer: ((choice: SequenceChoice) => void) | null = null;

/** The "Import as an image sequence?" question (`SequenceOffer.tsx` shows it). */
export const sequenceOfferStore = createStore<SequenceOfferStore>()((set) => ({
  offer: null,
  ask: (run) => {
    resolveOffer?.("cancel");
    set({ offer: run });
    return new Promise<SequenceChoice>((resolve) => {
      resolveOffer = resolve;
    });
  },
  answer: (choice) => {
    const resolve = resolveOffer;
    resolveOffer = null;
    set({ offer: null });
    resolve?.(choice);
  },
}));

export interface PickedImportOptions extends ImportOptions, SequenceImportOptions {}

/**
 * Imports the files the Import… picker returned, or that were dropped from the OS. Files that
 * are a numbered run of pictures are asked about first: as one image sequence, as separate pictures, or not at all. Resolves like
 * `importMedia` (a sequence counts as one asset added), or `null` when nothing was imported.
 */
export async function importPicked(
  deps: ImportDeps,
  paths: readonly string[],
  folder: FolderId | null,
  options: PickedImportOptions,
): Promise<ImportSummary | null> {
  const run = runOfSelection(paths);
  if (run === null) return importMedia(deps, paths, folder, options);
  const choice = await sequenceOfferStore.getState().ask(run);
  if (choice === "cancel") return null;
  if (choice === "separate") return importMedia(deps, paths, folder, options);
  const id = await importSequence(deps, run.path, folder, options);
  if (id === null) return null;
  return {
    added: 1,
    duplicates: 0,
    failed: [],
    folders: 0,
    skipped: { unsupported: 0, unreadable: 0, tooDeep: 0 },
  };
}
