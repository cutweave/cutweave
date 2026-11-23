// SPDX-License-Identifier: GPL-3.0-or-later
// Binds `ProjectController` to the app: one controller on the global stores, the keyboard
// shortcuts (undo, redo, save, open, new) when there is no native menu to deliver them, and the
// window title.
import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { translator } from "../timeline/errors.ts";
import { desktopFromWindow } from "./desktop.ts";
import { ProjectController } from "./projectController.ts";
import { hasUnsavedWork, sessionStore, useSessionStore } from "./sessionStore.ts";
import { sessionUiStore } from "./sessionUiStore.ts";

let shared: ProjectController | null = null;

/** The app's controller, created on first use over the global stores. */
export function projectController(): ProjectController {
  shared ??= new ProjectController({
    engine: engineStore,
    session: sessionStore,
    doc: docStore,
    ui: sessionUiStore,
    toasts: toastStore,
    t: translator((key: string, vars?: object) => i18n.t(key as never, vars as never)),
    desktop: desktopFromWindow(),
  });
  return shared;
}

/** Forgets the shared controller so the next use builds one again (tests that swap the desktop bridge). */
export function resetProjectController(): void {
  shared?.detach();
  shared = null;
}

/** Keys typed into a field belong to the field (its own undo, its own Enter). */
function isTextEntry(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (el === null || typeof el.tagName !== "string") return false;
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA";
}

/**
 * Wires the controller to the desktop shell and, without one, to the keyboard. Mount once in the
 * shell. In the desktop app the menu's accelerators deliver the same commands; a text field that
 * has the focus keeps Ctrl+Z for its own text.
 */
export function useProjectWiring(): void {
  useEffect(() => {
    const controller = projectController();
    const detach = controller.attach();
    const native = desktopFromWindow()?.menu !== undefined;
    const onKey = (event: KeyboardEvent): void => {
      if (native || event.defaultPrevented) return;
      if (!(event.ctrlKey || event.metaKey) || event.altKey) return;
      const key = event.key.toLowerCase();
      const typing = isTextEntry(event.target);
      let handled = true;
      if (key === "z" && !typing) void (event.shiftKey ? controller.redo() : controller.undo());
      else if (key === "y" && !typing && !event.metaKey) void controller.redo();
      else if (key === "s") void (event.shiftKey ? controller.saveAs() : controller.save());
      else if (key === "o") void controller.open();
      else if (key === "n") void controller.newProject();
      else handled = false;
      if (handled) event.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      detach();
    };
  }, []);
}

/** Whether leaving now would lose work: the dot next to Save and in the window title. */
export function useUnsavedWork(): boolean {
  const session = useSessionStore((s) => s);
  const project = useDocStore((s) => s.project);
  return hasUnsavedWork(session, project);
}

/** The window title: the project name, a bullet while there are unsaved changes. */
export function useWindowTitle(): void {
  const { t } = useTranslation();
  const known = useSessionStore((s) => s.known);
  const dirty = useUnsavedWork();
  const engineName = useSessionStore((s) => s.name);
  const docName = useDocStore((s) => s.project?.meta.name ?? null);
  const name = docName ?? (engineName === "" ? t("project.untitled") : engineName);
  useEffect(() => {
    // Without an engine (plain `vite` dev) there is no project to name.
    document.title = known
      ? t(dirty ? "project.windowTitleDirty" : "project.windowTitle", { name })
      : t("app.windowTitle");
  }, [t, known, dirty, name]);
}
