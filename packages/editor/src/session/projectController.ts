// SPDX-License-Identifier: GPL-3.0-or-later
// Project actions of the editor (WP-1M.07): new, open, save, save as, close with unsaved
// changes, crash recovery, undo and redo. Framework-free: the flows run against a real engine in
// Vitest and `ProjectDialogs` / `TopBar` bind them to React. Everything goes through engine
// commands (arch §5.1); this class holds no document state. Whether the project is dirty is
// the engine's `project.dirty` event in `sessionStore`.
//
// Leaving a project with unsaved changes asks first (Save / Discard / Cancel). "Discard" closes
// with `save: false` so the engine's autosave-on-close cannot write them either; the work stays
// in the journal and is offered again by the recovery dialog the next time the project opens.
// Open passes `discard_unsaved: true` instead, and only after the person chose Discard
// (ADR-0029); when the engine refuses an open for changes the editor did not count, it asks then.

import { type ProjectStatus, RpcError } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DocStore } from "../state/docStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";
import { applyNewProjectWorkspace } from "../workspace/newProject.ts";
import { type DesktopBridge, isSessionCommand, type SessionMenuCommand } from "./desktop.ts";
import { asRpcError, describeProjectError, isUnsavedChanges } from "./errors.ts";
import { stepName } from "./historyLabels.ts";
import { hasUnsavedWork, type SessionStore } from "./sessionStore.ts";
import {
  joinProjectPath,
  type ProjectTarget,
  type SessionUiStore,
  type UnsavedChoice,
} from "./sessionUiStore.ts";

export interface ProjectControllerDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly session: StoreApi<SessionStore>;
  readonly doc: StoreApi<DocStore>;
  readonly ui: StoreApi<SessionUiStore>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly desktop: DesktopBridge | null;
}

/** Whether a text input, text area or editable element has the focus. */
function textFieldFocused(): boolean {
  if (typeof document === "undefined") return false;
  const el = document.activeElement as HTMLElement | null;
  if (el === null) return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA";
}

export class ProjectController {
  readonly #deps: ProjectControllerDeps;
  #offs: (() => void)[] = [];

  constructor(deps: ProjectControllerDeps) {
    this.#deps = deps;
  }

  // ---- wiring -------------------------------------------------------------------------------

  /**
   * Connects the desktop shell: native menu commands, the recent list, the window-close
   * handshake and the dirty flag main needs for it. Returns the disconnect function.
   */
  attach(): () => void {
    const { desktop, session } = this.#deps;
    if (desktop !== null) {
      void this.refreshRecent();
      if (desktop.menu !== undefined) {
        this.#offs.push(
          desktop.menu.onCommand((command) => {
            if (isSessionCommand(command)) void this.runMenuCommand(command);
          }),
        );
      }
      const win = desktop.window;
      if (win !== undefined) {
        // Main holds a close request while this is true (unsaved work, saved or not yet titled).
        let reported: boolean | null = null;
        const report = (): void => {
          const unsaved = this.hasUnsavedWork();
          if (unsaved === reported) return;
          reported = unsaved;
          void win.setDirty(unsaved).catch(() => undefined);
        };
        report();
        this.#offs.push(
          session.subscribe(report),
          this.#deps.doc.subscribe(report),
          win.onCloseRequested(() => void this.requestClose()),
        );
      }
    }
    return () => this.detach();
  }

  detach(): void {
    for (const off of this.#offs) off();
    this.#offs = [];
  }

  /**
   * Runs a command chosen from a menu (the native menu on macOS, the title bar's menu bar
   * elsewhere) or its shortcut. A text field that has the focus keeps Undo and Redo for its own
   * text, as it would with a native menu.
   */
  runMenuCommand(command: SessionMenuCommand): Promise<unknown> {
    if ((command === "undo" || command === "redo") && textFieldFocused()) {
      document.execCommand(command);
      return Promise.resolve(false);
    }
    return this.run(command);
  }

  /** Runs a command a menu item, shortcut or button names. */
  run(command: "new" | "open" | "save" | "saveAs" | "undo" | "redo"): Promise<unknown> {
    switch (command) {
      case "new":
        return this.newProject();
      case "open":
        return this.open();
      case "save":
        return this.save();
      case "saveAs":
        return this.saveAs();
      case "undo":
        return this.undo();
      case "redo":
        return this.redo();
    }
  }

  // ---- helpers ------------------------------------------------------------------------------

  /** The document's own name, then the engine's, then "Untitled project". */
  projectName(): string {
    const { doc, session, t } = this.#deps;
    const named = doc.getState().project?.meta.name;
    if (named !== undefined && named !== "") return named;
    const engine = session.getState().name;
    return engine === "" ? t("projectUi.untitled") : engine;
  }

  /** A command over the editor's connection (results are cast by each flow). */
  #call(name: string, input: object): Promise<unknown> {
    const { rpc, blocked } = this.#deps.engine.getState();
    if (rpc === null) return Promise.reject(new RpcError("unsupported", "no engine is connected"));
    if (blocked !== null) {
      return Promise.reject(new RpcError("unsupported", "the engine is not available"));
    }
    return (rpc as unknown as { call(name: string, input: unknown): Promise<unknown> }).call(
      name,
      input,
    );
  }

  async #busy<T>(work: () => Promise<T>): Promise<T> {
    this.#deps.ui.getState().setBusy(true);
    try {
      return await work();
    } finally {
      this.#deps.ui.getState().setBusy(false);
    }
  }

  #fail(title: Parameters<Translate>[0], error: unknown): void {
    const failure = asRpcError(error);
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t(title),
      message: describeProjectError(failure, this.#deps.t),
    });
  }

  /** A refused Undo or Redo: a conflict says in words what could not be done and why. */
  #failHistory(kind: "undo" | "redo", error: unknown): void {
    const failure = asRpcError(error);
    if (failure.code !== "conflict" || failure.data?.reason !== "history_target_modified") {
      this.#fail(kind === "undo" ? "projectUi.failed.undo" : "projectUi.failed.redo", error);
      return;
    }
    const stacks = this.#deps.session.getState().stacks;
    const step =
      kind === "undo"
        ? stepName(stacks.nextUndo ?? "", stacks.nextUndoActor, this.#deps.t)
        : stepName(stacks.nextRedo ?? "", stacks.nextRedoActor, this.#deps.t);
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t(kind === "undo" ? "projectUi.failed.undo" : "projectUi.failed.redo"),
      message: this.#deps.t(
        kind === "undo" ? "projectUi.historyConflict.undo" : "projectUi.historyConflict.redo",
        { step },
      ),
    });
  }

  async refreshRecent(): Promise<void> {
    const recent = this.#deps.desktop?.recent;
    if (recent === undefined) return;
    try {
      this.#deps.ui.getState().setRecent(await recent.list());
    } catch {
      // The list is a convenience; the menu just stays as it was.
    }
  }

  async #remember(path: string): Promise<void> {
    const recent = this.#deps.desktop?.recent;
    if (recent === undefined) return;
    try {
      await recent.add(path);
    } catch {
      // Not remembering a path never blocks the project.
    }
    await this.refreshRecent();
  }

  #applyStatus(status: ProjectStatus): void {
    this.#deps.session.getState().applyStatus(status);
  }

  // ---- unsaved changes ----------------------------------------------------------------------

  /** Asks Save / Discard / Cancel ("Save changes to <name>?"). */
  #askUnsaved(): Promise<UnsavedChoice> {
    const { ui } = this.#deps;
    return new Promise<UnsavedChoice>((resolve) => {
      ui.getState().setUnsaved({
        project: this.projectName(),
        resolve: (answer) => {
          ui.getState().setUnsaved(null);
          resolve(answer);
        },
      });
    });
  }

  /**
   * Settles unsaved changes before an open, which drops them itself when told to (ADR-0029):
   * `go` (nothing unsaved, or saved), `discard` (the person chose to drop them: open with
   * `discard_unsaved`) or `stay`. `ask` asks even when the editor counts nothing unsaved (the
   * engine refused the open for changes it knows of).
   */
  async #settleForOpen(ask = false): Promise<"go" | "discard" | "stay"> {
    if (!ask && !this.hasUnsavedWork()) return "go";
    const choice = await this.#askUnsaved();
    if (choice === "cancel") return "stay";
    if (choice === "save") return (await this.save()) ? "go" : "stay";
    return "discard";
  }

  /**
   * Settles unsaved changes before something that would lose them. `true`: go on (saved, or
   * the person chose to discard: the project is then closed without saving); `false`: stay.
   */
  async settleUnsaved(): Promise<boolean> {
    if (!this.hasUnsavedWork()) return true;
    const choice = await this.#askUnsaved();
    if (choice === "cancel") return false;
    if (choice === "save") return this.save();
    try {
      const out = (await this.#call("project.close", { save: false })) as {
        project: ProjectStatus;
      };
      this.#applyStatus(out.project);
      return true;
    } catch (error) {
      this.#fail("projectUi.failed.save", error);
      return false;
    }
  }

  /** Whether leaving now would lose work (see `hasUnsavedWork` in `sessionStore`). */
  hasUnsavedWork(): boolean {
    return hasUnsavedWork(this.#deps.session.getState(), this.#deps.doc.getState().project);
  }

  /** The desktop shell asks to close the window; a dirty project is settled first. */
  async requestClose(): Promise<void> {
    if (!(await this.settleUnsaved())) return;
    await this.#deps.desktop?.window?.confirmClose();
  }

  // ---- new, open, save ----------------------------------------------------------------------

  /** Asks for a name and a place; `null` when cancelled. */
  #askTarget(mode: "new" | "saveAs", initialName: string): Promise<ProjectTarget | null> {
    const { ui } = this.#deps;
    return new Promise((resolve) => {
      ui.getState().setNaming({
        mode,
        initialName,
        resolve: (target) => {
          ui.getState().setNaming(null);
          resolve(target);
        },
      });
    });
  }

  /** The folder picker; the dialog calls it for its "Choose folder" button. */
  async chooseFolder(): Promise<string | null> {
    return (await this.#deps.desktop?.dialog.openProjectFolder()) ?? null;
  }

  /** New project: name and place, then an empty project saved to its folder. */
  async newProject(): Promise<boolean> {
    if (!(await this.settleUnsaved())) return false;
    const target = await this.#askTarget("new", "");
    if (target === null) return false;
    return this.#busy(async () => {
      try {
        const closed = (await this.#call("project.close", { save: false })) as {
          project: ProjectStatus;
        };
        this.#applyStatus(closed.project);
        await this.#call("project.new", { name: target.name });
        const saved = (await this.#call("project.save_as", { path: target.path })) as {
          path: string;
        };
        this.#deps.session.getState().applySaved(saved.path);
        await this.#remember(saved.path);
        applyNewProjectWorkspace(); // Settings > General > Workspace for new projects (WP-2H.01)
        return true;
      } catch (error) {
        this.#fail("projectUi.failed.create", error);
        return false;
      }
    });
  }

  /**
   * "On startup: open the last project" (Settings > General): opens the newest recent project.
   * Resolves `false` when there is none or it cannot be opened (the person is told, as for any
   * open).
   */
  async openLastOnStartup(): Promise<boolean> {
    const recent = this.#deps.desktop?.recent;
    if (recent === undefined) return false;
    let list: readonly string[];
    try {
      list = await recent.list();
    } catch {
      return false;
    }
    const last = list[0];
    return last === undefined ? false : this.open(last);
  }

  /**
   * Opens a project: `path` when given (the recent list), else the native folder picker. Unsaved
   * changes are settled first; the engine drops them only when the person chose Discard.
   */
  async open(path?: string): Promise<boolean> {
    const chosen = path ?? (await this.chooseFolder());
    if (chosen === null) return false;
    let settled = await this.#settleForOpen();
    for (;;) {
      if (settled === "stay") return false;
      const discard = settled === "discard";
      const outcome = await this.#busy(async () => {
        try {
          const out = (await this.#call(
            "project.open",
            discard ? { path: chosen, discard_unsaved: true } : { path: chosen },
          )) as {
            project: ProjectStatus;
            missing_media: string[];
            migrated_from: number | null;
          };
          this.#applyStatus(out.project);
          await this.#remember(out.project.path ?? chosen);
          this.#reportOpened(out);
          return "opened" as const;
        } catch (error) {
          // Changes the editor did not count (or that came in since): ask now, then again.
          if (!discard && isUnsavedChanges(error)) return "unsaved" as const;
          this.#fail("projectUi.failed.open", error);
          if (path !== undefined) void this.#forget(path);
          return "failed" as const;
        }
      });
      if (outcome !== "unsaved") return outcome === "opened";
      settled = await this.#settleForOpen(true);
    }
  }

  async #forget(path: string): Promise<void> {
    try {
      await this.#deps.desktop?.recent?.remove(path);
    } catch {
      // See `#remember`.
    }
    await this.refreshRecent();
  }

  #reportOpened(out: {
    project: ProjectStatus;
    missing_media: readonly string[];
    migrated_from: number | null;
  }): void {
    const { t, toasts } = this.#deps;
    const count = out.missing_media.length;
    if (count > 0) {
      toasts.getState().push({
        tone: "warning",
        title: t("projectUi.missingMediaTitle"),
        message:
          count === 1
            ? t("projectUi.missingMediaOne")
            : t("projectUi.missingMedia", { count: String(count) }),
      });
    }
    if (out.migrated_from !== null) {
      toasts.getState().push({
        tone: "info",
        title: t("projectUi.migratedTitle"),
        message: t("projectUi.migrated", {
          name: out.project.name,
          from: String(out.migrated_from),
        }),
      });
    }
  }

  /** Saves; an untitled document asks where first (`saveAs`). `true` when it is saved. */
  async save(): Promise<boolean> {
    const { session } = this.#deps;
    const state = session.getState();
    if (state.readOnly !== null) {
      this.#fail("projectUi.failed.save", new RpcError("unsupported", "the project is read-only"));
      return false;
    }
    if (state.path === null) return this.saveAs();
    return this.#busy(async () => {
      this.#deps.ui.getState().setSaving(true);
      try {
        await this.#call("project.save", {});
        return true;
      } catch (error) {
        this.#fail("projectUi.failed.save", error);
        return false;
      } finally {
        this.#deps.ui.getState().setSaving(false);
      }
    });
  }

  /** Save as: name and place, then the document continues in the new project folder. */
  async saveAs(): Promise<boolean> {
    const target = await this.#askTarget("saveAs", this.projectName());
    if (target === null) return false;
    return this.#busy(async () => {
      try {
        const out = (await this.#call("project.save_as", { path: target.path })) as {
          path: string;
        };
        this.#deps.session.getState().applySaved(out.path);
        await this.#remember(out.path);
        return true;
      } catch (error) {
        this.#fail("projectUi.failed.save", error);
        return false;
      }
    });
  }

  /** The target the naming dialog resolves with. */
  target(name: string, parent: string): ProjectTarget {
    const trimmed = name.trim();
    return { name: trimmed, parent, path: joinProjectPath(parent, trimmed) };
  }

  // ---- recovery -----------------------------------------------------------------------------

  /** Restore: replays the journal onto the saved document (`project.recover`). */
  async recover(): Promise<boolean> {
    const { t, toasts } = this.#deps;
    return this.#busy(async () => {
      try {
        const out = (await this.#call("project.recover", {})) as {
          project: ProjectStatus;
          applied: number;
          damaged: boolean;
        };
        this.#applyStatus(out.project);
        toasts.getState().push({
          tone: "success",
          title: t("projectUi.recovered.title"),
          message: t("projectUi.recovered.body", { count: String(out.applied) }),
        });
        return true;
      } catch (error) {
        this.#fail("projectUi.failed.recover", error);
        return false;
      }
    });
  }

  /** Discard: keeps the project as it was last saved (`project.discard_recovery`). */
  async discardRecovery(): Promise<boolean> {
    return this.#busy(async () => {
      try {
        const out = (await this.#call("project.discard_recovery", {})) as {
          project: ProjectStatus;
        };
        this.#applyStatus(out.project);
        return true;
      } catch (error) {
        this.#fail("projectUi.failed.discard", error);
        return false;
      }
    });
  }

  // ---- undo and redo ------------------------------------------------------------------------

  async undo(): Promise<boolean> {
    if (this.#deps.session.getState().stacks.undoDepth === 0) return false;
    try {
      await this.#call("edit.undo", { actor: "*" });
      return true;
    } catch (error) {
      this.#failHistory("undo", error);
      return false;
    }
  }

  async redo(): Promise<boolean> {
    if (this.#deps.session.getState().stacks.redoDepth === 0) return false;
    try {
      await this.#call("edit.redo", { actor: "*" });
      return true;
    } catch (error) {
      this.#failHistory("redo", error);
      return false;
    }
  }
}
