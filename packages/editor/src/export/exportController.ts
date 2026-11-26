// SPDX-License-Identifier: GPL-3.0-or-later
// The export flow (WP-1M.08, WP-2G.04, arch §7 "Export"): reads the presets and encoders the
// engine offers, resolves a preset for the sequence (size, rate, estimate, warnings), starts
// `export.start` (Export) or `export.enqueue` (Add to queue), follows the started jobs through
// `jobStore` (the engine's `job.progress` events), cancels them with `job.cancel`, and says how
// they ended with a toast. The dialog and the top bar's chip only read `exportStore` and
// `jobStore`, so the export carries on when the dialog is hidden.
//
// Every engine call goes over the editor's own connection; nothing here touches the document.

import { defaultPreferences, type PreferenceKey, type PreferenceMap } from "@cutweave/preferences";
import type {
  AdvancedExportOptions,
  Collision,
  Container,
  CustomExport,
  ExportedFile,
  ExportHw,
  ExportPreset,
  ExportQuality,
  ExportRange,
  ExportStartOutput,
  JobInfo,
  PresetWarning,
  Project,
  ResolvePresetOutput,
  RpcClient,
  Sequence,
  SequenceId,
  VideoCodec,
} from "@cutweave/protocol";
import type { JsonSchema } from "@cutweave/ui";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { PreferencesClient } from "../prefs/prefsStore.ts";
import type { DesktopBridge } from "../session/desktop.ts";
import { asRpcError } from "../session/errors.ts";
import type { SessionStore } from "../session/sessionStore.ts";
import { isActive, type JobStore } from "../state/jobStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";
import { dateToken, expandFileName, humanBytes, withExtension } from "./exportModel.ts";
import {
  baseName,
  dirName,
  proposedOutputPath,
  type SequenceFacts,
  sequenceFacts,
} from "./exportSettings.ts";
import type { ActiveFile, ExportStore } from "./exportStore.ts";

export interface ExportControllerDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly project: () => Project | null;
  readonly session: StoreApi<SessionStore>;
  readonly jobs: StoreApi<JobStore>;
  readonly state: StoreApi<ExportStore>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly desktop: DesktopBridge | null;
  /** The person's export defaults (Settings > Export); without it the schema's defaults stand. */
  readonly prefs?: Pick<PreferencesClient, "get" | "set">;
  /** Shows the export queue panel ("Show queue" on the queued toast). */
  readonly showQueue?: () => void;
  /** Today, for the `{date}` of the file name pattern. */
  readonly now?: () => Date;
}

/** The sequence an export takes (the one the timeline shows), with the facts the dialog shows. */
export interface ExportSource {
  readonly sequenceId: SequenceId;
  readonly sequence: Sequence;
  readonly facts: SequenceFacts;
  readonly hasItems: boolean;
}

/** The sequence the timeline shows (else the project's first), or `null` when there is none. */
export function exportSource(project: Project | null): ExportSource | null {
  const id = selectActiveSequenceId(project);
  const sequence =
    (id === null ? undefined : project?.sequences[id]) ??
    Object.values(project?.sequences ?? {})[0];
  if (sequence === undefined) return null;
  const hasItems = Object.values(sequence.tracks).some(
    (track) => Object.keys(track.items).length > 0,
  );
  return { sequenceId: sequence.id, sequence, facts: sequenceFacts(sequence), hasItems };
}

/** What the dialog asks the controller to start. */
export interface ExportRequest {
  readonly path: string;
  /** A preset id, or `{custom: {preset, …}}` with the person's changes to it. */
  readonly preset: ExportPreset;
  /** The preset's id, remembered as the last one used. */
  readonly presetId?: string | undefined;
  readonly hw: ExportHw;
  /** Absent: ask before replacing a file (the engine refuses at `/overwrite`). */
  readonly collision?: Collision | undefined;
  readonly range?: ExportRange | undefined;
  readonly advanced?: AdvancedExportOptions | null | undefined;
}

/** Export now (`export.start`) or add it to the queue (`export.enqueue`). */
export type StartMode = "now" | "queue";

export type StartOutcome =
  | {
      readonly kind: "started";
      readonly files: readonly ActiveFile[];
      readonly warnings: readonly PresetWarning[];
      readonly estimatedKib: number | null;
    }
  /** The file is already there: ask before replacing it. */
  | { readonly kind: "exists" }
  /** The engine refused; `pointer` says which field (`/path`, `/range/kind`, `/advanced/…`). */
  | { readonly kind: "refused"; readonly message: string; readonly pointer: string | null };

export type ResolveOutcome =
  | { readonly ok: true; readonly value: ResolvePresetOutput }
  | { readonly ok: false; readonly message: string; readonly pointer: string | null };

/** How long the "Reveal in folder" toast stays. */
export const DONE_TOAST_MS = 12_000;

/** The files of a start answer: every file of a marker set, else the one file. */
export function startedFiles(out: ExportStartOutput): ActiveFile[] {
  const list: readonly Pick<ExportedFile, "job_id" | "path">[] =
    out.jobs !== undefined && out.jobs.length > 0 ? out.jobs : [out];
  return list.map((f) => ({ jobId: f.job_id, path: f.path, fileName: baseName(f.path) }));
}

export class ExportController {
  readonly #deps: ExportControllerDeps;
  #offs: (() => void)[] = [];
  readonly #schemas = new Map<string, Promise<JsonSchema | null>>();

  constructor(deps: ExportControllerDeps) {
    this.#deps = deps;
  }

  // ---- wiring -------------------------------------------------------------------------------

  /** Follows the jobs and the native menu's Export… (macOS). Returns the disconnect function. */
  attach(): () => void {
    const { jobs, desktop } = this.#deps;
    this.#offs.push(jobs.subscribe(() => this.#settle()));
    if (desktop?.menu !== undefined) {
      this.#offs.push(
        desktop.menu.onCommand((command) => {
          if (command === "export") this.open();
        }),
      );
    }
    return () => this.detach();
  }

  detach(): void {
    for (const off of this.#offs.splice(0)) off();
  }

  // ---- the dialog ---------------------------------------------------------------------------

  /** Opens the dialog (the progress view while an export runs); does nothing with an empty timeline. */
  open(): void {
    const { state } = this.#deps;
    if (state.getState().active === null && !this.available()) return;
    state.getState().setDialogOpen(true);
    if (state.getState().active === null) {
      void this.loadPresets();
      void this.loadEncoders();
    }
  }

  close(): void {
    this.#deps.state.getState().setDialogOpen(false);
  }

  /** Whether there is something to export and an engine to do it. */
  available(): boolean {
    return exportSource(this.#deps.project())?.hasItems === true;
  }

  #pref<K extends PreferenceKey>(key: K): PreferenceMap[K] {
    return this.#deps.prefs?.get(key) ?? defaultPreferences()[key];
  }

  #rpc(): RpcClient | null {
    const { rpc, blocked } = this.#deps.engine.getState();
    return rpc === null || blocked !== null ? null : rpc;
  }

  /** What a new dialog starts at (Settings > Export). */
  formDefaults(): {
    readonly quality: ExportQuality;
    readonly hardware: boolean;
    /** The presets to try, in order: the default one, then the last one used. */
    readonly presets: readonly string[];
    readonly fileName: string;
  } {
    const chosen = this.#pref("export.preset");
    return {
      quality: this.#pref("export.quality"),
      hardware: this.#pref("export.hw") === "auto",
      presets: chosen === "" ? [this.#pref("export.last_preset")] : [chosen],
      fileName: this.#pref("export.filename"),
    };
  }

  /** The project's name, else the session's, else "Export". */
  #projectName(): string {
    const named = this.#deps.project()?.meta.name;
    return named !== undefined && named !== ""
      ? named
      : this.#deps.session.getState().name || "Export";
  }

  /** The file name the pattern of Settings > Export gives, without the extension. */
  fileStem(presetName: string): string {
    const source = exportSource(this.#deps.project());
    return expandFileName(this.#pref("export.filename"), {
      project: this.#projectName(),
      sequence: source?.sequence.name ?? "",
      preset: presetName,
      date: dateToken(this.#deps.now?.() ?? new Date()),
    });
  }

  /**
   * Where the file goes unless the person picks another place: in the project's folder, the
   * last folder used or the chosen one (Settings > Export), named by the file name pattern,
   * with the preset's extension.
   */
  defaultPath(presetName = "", container: Container = "mp4"): string {
    const path = proposedOutputPath(
      this.#pref("export.folder"),
      { last: this.#pref("export.last_folder"), chosen: this.#pref("export.folder_path") },
      this.#deps.session.getState().path,
      this.fileStem(presetName),
    );
    return withExtension(path, container);
  }

  /** The native "save as" for the file: the chosen path, or `null` (cancelled or no desktop). */
  async chooseFile(current: string, container: Container = "mp4"): Promise<string | null> {
    const pick = this.#deps.desktop?.dialog.chooseExportFile;
    if (pick === undefined) return null;
    try {
      const picked = await pick(current === "" ? this.defaultPath("", container) : current);
      // Another folder in the dialog is "the last folder used"; the chosen one stays as set.
      const folder = picked === null ? null : dirName(picked);
      if (folder !== null && folder !== this.#pref("export.last_folder")) {
        void this.#deps.prefs?.set("export.last_folder", folder).catch(() => undefined);
      }
      // The OS dialog offers MP4; a preset that writes another container keeps its own.
      return picked === null ? null : withExtension(picked, container);
    } catch {
      return null;
    }
  }

  /** Asks the engine for its presets (built-in and the person's), once per dialog. */
  async loadPresets(): Promise<void> {
    const { state } = this.#deps;
    const status = state.getState().presets.status;
    if (status === "loading") return;
    const rpc = this.#rpc();
    if (rpc === null) return;
    if (status !== "ready") state.getState().setPresets({ status: "loading" });
    try {
      const answer = await rpc.call("export.list_presets", {});
      state.getState().setPresets({ status: "ready", value: answer.presets });
    } catch {
      if (state.getState().presets.status !== "ready") {
        state.getState().setPresets({ status: "unavailable" });
      }
    }
  }

  /**
   * Asks the engine which encoders each codec has here. The first answer pays for the hardware
   * trial encodes (about a second), so it is asked as the dialog opens, before Export is pressed.
   * An engine without the query leaves the encoder unnamed; "auto" still picks one.
   */
  async loadEncoders(): Promise<void> {
    const { state } = this.#deps;
    const status = state.getState().encoders.status;
    if (status === "loading" || status === "ready") return;
    const rpc = this.#rpc();
    if (rpc === null) return;
    state.getState().setEncoders({ status: "loading" });
    try {
      const answer = await rpc.call("export.list_encoders", {});
      state.getState().setEncoders({ status: "ready", value: answer.codecs });
    } catch {
      state.getState().setEncoders({ status: "unavailable" });
    }
  }

  /** What a preset writes for this sequence and range: size, rate, estimate and warnings. */
  async resolve(
    preset: string,
    range: ExportRange,
    overrides: CustomExport | null,
  ): Promise<ResolveOutcome> {
    const source = exportSource(this.#deps.project());
    const rpc = this.#rpc();
    if (source === null || rpc === null) {
      return { ok: false, message: this.#deps.t("export.errors.engine"), pointer: null };
    }
    try {
      const value = await rpc.call("export.resolve_preset", {
        preset,
        sequence: source.sequenceId,
        range,
        overrides,
      });
      return { ok: true, value };
    } catch (error) {
      const failure = asRpcError(error);
      return { ok: false, message: failure.message, pointer: failure.path ?? null };
    }
  }

  /** The JSON Schema of the encoder options for a codec in a container (kept per codec). */
  describeOptions(codec: VideoCodec, container: Container): Promise<JsonSchema | null> {
    const key = `${codec}/${container}`;
    let found = this.#schemas.get(key);
    if (found === undefined) {
      const rpc = this.#rpc();
      if (rpc === null) return Promise.resolve(null);
      found = rpc
        .call("export.describe_options", { codec, container })
        .then((out) => out.schema as JsonSchema)
        .catch(() => {
          this.#schemas.delete(key);
          return null;
        });
      this.#schemas.set(key, found);
    }
    return found;
  }

  // ---- running it ---------------------------------------------------------------------------

  /** Starts the export now, or queues it. Resolves with what the dialog should do next. */
  async start(request: ExportRequest, mode: StartMode = "now"): Promise<StartOutcome> {
    const { state, t } = this.#deps;
    const source = exportSource(this.#deps.project());
    if (source === null || !source.hasItems) {
      return {
        kind: "refused",
        message: t("export.errors.start", { message: "—" }),
        pointer: null,
      };
    }
    const rpc = this.#rpc();
    if (rpc === null) {
      return { kind: "refused", message: t("export.errors.engine"), pointer: null };
    }
    const params = {
      sequence: source.sequenceId,
      path: request.path,
      preset: request.preset,
      hw: request.hw,
      ...(request.collision === undefined
        ? { overwrite: false }
        : { collision: request.collision }),
      ...(request.range === undefined ? {} : { range: request.range }),
      ...(request.advanced === undefined || request.advanced === null
        ? {}
        : { advanced: request.advanced }),
    };
    try {
      const out =
        mode === "now"
          ? await rpc.call("export.start", params)
          : await rpc.call("export.enqueue", params);
      const files = startedFiles(out);
      const estimatedKib = out.estimated_kib ?? null;
      if (request.presetId !== undefined) this.#remember(request.presetId);
      if (mode === "now") {
        state.getState().setActive({ files, seen: false, warnings: out.warnings, estimatedKib });
        // The first `job.progress` may have arrived before this answer did.
        this.#settle();
      } else {
        this.#queued(files, estimatedKib);
      }
      return { kind: "started", files, warnings: out.warnings, estimatedKib };
    } catch (error) {
      const failure = asRpcError(error);
      if (failure.code === "invalid" && failure.path?.endsWith("/overwrite") === true) {
        return { kind: "exists" };
      }
      return { kind: "refused", message: failure.message, pointer: failure.path ?? null };
    }
  }

  #remember(presetId: string): void {
    if (presetId === "" || this.#pref("export.last_preset") === presetId) return;
    void this.#deps.prefs?.set("export.last_preset", presetId).catch(() => undefined);
  }

  /** "Added to the queue", with Show queue. */
  #queued(files: readonly ActiveFile[], estimatedKib: number | null): void {
    const { toasts, t, showQueue } = this.#deps;
    const names =
      files.length === 1
        ? (files[0]?.fileName ?? "")
        : t("export.files", { count: String(files.length) });
    toasts.getState().push({
      tone: "info",
      title: t("export.queued.title"),
      message:
        estimatedKib === null
          ? t("export.queued.message", { name: names })
          : t("export.queued.sized", { name: names, size: humanBytes(estimatedKib * 1024) }),
      ...(showQueue === undefined
        ? {}
        : { actions: [{ label: t("export.queued.show"), onClick: () => showQueue() }] }),
    });
  }

  /** Asks the engine to stop the export; the jobs end `cancelled` and no file is left. */
  async cancel(): Promise<void> {
    const { engine, state, jobs } = this.#deps;
    const active = state.getState().active;
    const rpc: RpcClient | null = engine.getState().rpc;
    if (active === null || rpc === null) return;
    state.getState().setCancelling(true);
    const running = active.files.filter((f) => {
      const info = jobs.getState().jobs[f.jobId];
      return info === undefined || isActive(info);
    });
    try {
      for (const file of running) {
        const info = await rpc.call("job.cancel", { id: file.jobId });
        jobs.getState().applyProgress(info);
      }
    } catch (error) {
      state.getState().setCancelling(false);
      this.#deps.toasts.getState().push({
        tone: "danger",
        title: this.#deps.t("export.failed.title"),
        message: asRpcError(error).message,
      });
    }
  }

  // ---- how it ended -------------------------------------------------------------------------

  /** Looks at the jobs we follow: marks them seen, and reports how they ended. */
  #settle(): void {
    const { state, jobs } = this.#deps;
    const active = state.getState().active;
    if (active === null) return;
    const infos = active.files.map((f) => jobs.getState().jobs[f.jobId]);
    if (infos.some((info) => info === undefined)) {
      // They were in the list and one is gone: the engine restarted and knows nothing of it.
      if (active.seen) this.#lost();
      return;
    }
    if (!active.seen) state.getState().markSeen();
    const known = infos as JobInfo[];
    if (known.every((info) => !isActive(info))) this.#finish(active.files, known);
  }

  #end(): void {
    const { state } = this.#deps;
    state.getState().setActive(null);
    state.getState().setDialogOpen(false);
  }

  #finish(files: readonly ActiveFile[], infos: readonly JobInfo[]): void {
    const { toasts, t, desktop } = this.#deps;
    this.#end();
    const failed = infos.find((info) => info.state === "failed");
    const succeeded = infos.filter((info) => info.state === "succeeded");
    if (failed !== undefined) {
      toasts.getState().push({
        tone: "danger",
        title: t("export.failed.title"),
        message: failed.error ?? t("export.failed.unknown"),
      });
    } else if (succeeded.length === infos.length) {
      const reveal = desktop?.shell?.showItemInFolder;
      const first = files[0];
      const detail = infos[0]?.progress.message ?? null;
      const message =
        files.length > 1
          ? t("export.done.files", { count: String(files.length), name: first?.fileName ?? "" })
          : detail === null || detail === ""
            ? (first?.fileName ?? "")
            : `${first?.fileName ?? ""}: ${detail}`;
      toasts.getState().push({
        tone: "success",
        title: t("export.done.title"),
        message,
        duration: DONE_TOAST_MS,
        ...(reveal === undefined || first === undefined
          ? {}
          : {
              actions: [
                {
                  label: t("export.done.reveal"),
                  onClick: () => void reveal(first.path).catch(() => undefined),
                },
              ],
            }),
      });
    } else {
      toasts.getState().push({
        tone: "info",
        title: t("export.cancelled.title"),
        message: t("export.cancelled.message"),
      });
    }
  }

  #lost(): void {
    this.#end();
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t("export.lost.title"),
      message: this.#deps.t("export.lost.message"),
    });
  }
}
