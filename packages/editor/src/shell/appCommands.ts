// SPDX-License-Identifier: GPL-3.0-or-later
// Running the application menu's commands (WP-1M.09), from the menu bar and from the keyboard.
// Project commands go to `ProjectController`; quit, full screen, the edit commands, reload and
// the developer tools are main's (allowlisted IPC); About opens the editor's own dialog.
//
// On macOS the desktop app keeps the native menu, which delivers its accelerators itself, so the
// page takes no shortcuts there (`hasNativeMenu`).

import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { useEngineReady } from "../engine/engineStore.ts";
import { exportController, useTimelineHasItems } from "../export/useExport.ts";
import { openArchiveDialog } from "../media/ArchiveDialog.tsx";
import { openCollectDialog } from "../media/CollectDialog.tsx";
import { usePref } from "../prefs/prefsStore.ts";
import { openProjectSettings } from "../project/projectSettings.ts";
import { type DesktopBridge, desktopFromWindow, hasNativeMenu } from "../session/desktop.ts";
import { redoText, undoText } from "../session/historyLabels.ts";
import type { ProjectController } from "../session/projectController.ts";
import { projectFileName, useSessionStore } from "../session/sessionStore.ts";
import { sessionUiStore, useSessionUiStore } from "../session/sessionUiStore.ts";
import { isTextEntry } from "../session/useProject.ts";
import { settingsStore } from "../settings/settingsStore.ts";
import { translator } from "../timeline/errors.ts";
import { useOpenPanels } from "../workspace/dockHost.ts";
import { useRegisteredPanels } from "../workspace/panelRegistry.ts";
import {
  type AppCommand,
  type AppMenu,
  buildAppMenu,
  commandForKey,
  isEnabled,
  type MenuPlatform,
  menuPlatform,
} from "./appMenu.ts";
import { activeDocument, forEveryWindow, isMainWindow, windowOfEvent } from "./windows.ts";

/** Help > About is showing. */
export interface AboutState {
  readonly open: boolean;
  setOpen(open: boolean): void;
}

export const aboutStore = createStore<AboutState>()((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));

export function useAboutOpen(): boolean {
  return useStore(aboutStore, (s) => s.open);
}

// File > Settings… lives in `settings/settingsStore.ts` (it can open at a page and a row).

export interface AppCommandDeps {
  readonly controller: ProjectController;
  readonly desktop: DesktopBridge | null;
}

const ignore = (): undefined => undefined;

/** Runs one menu command. */
export function runAppCommand(command: AppCommand, deps: AppCommandDeps): void {
  const { controller, desktop } = deps;
  switch (command) {
    case "new":
    case "open":
    case "save":
    case "saveAs":
    case "undo":
    case "redo":
      void controller.runMenuCommand(command);
      return;
    case "export":
      exportController().open();
      return;
    case "collect":
      openCollectDialog();
      return;
    case "archive":
      openArchiveDialog();
      return;
    case "projectSettings":
      openProjectSettings();
      return;
    case "quit":
      void desktop?.app?.quit().catch(ignore);
      return;
    case "cut":
    case "copy":
    case "paste":
    case "selectAll":
      // Main runs them on the focused element, as the native edit roles do; the page alone
      // (plain `vite` dev) can still cut, copy and select.
      if (desktop?.edit !== undefined) void desktop.edit.run(command).catch(ignore);
      else activeDocument().execCommand(command);
      return;
    case "toggleFullScreen":
      void desktop?.window?.toggleFullScreen?.().catch(ignore);
      return;
    case "reload":
      void desktop?.window?.reload?.().catch(ignore);
      return;
    case "toggleDevTools":
      void desktop?.window?.toggleDevTools?.().catch(ignore);
      return;
    case "settings":
      settingsStore.getState().setOpen(true);
      return;
    case "about":
      aboutStore.getState().setOpen(true);
      return;
  }
}

/** The platform the page runs on, as the menus name it. */
export function currentMenuPlatform(): MenuPlatform {
  return menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
}

/** Whether this is an unpackaged desktop build (`app.getInfo().dev`); false until known. */
function useDevBuild(desktop: DesktopBridge | null): boolean {
  const [dev, setDev] = useState(false);
  useEffect(() => {
    let alive = true;
    desktop?.app
      ?.getInfo()
      .then((info) => {
        if (alive) setDev(info.dev === true);
      })
      .catch(ignore);
    return () => {
      alive = false;
    };
  }, [desktop]);
  return dev;
}

/** File, Edit, View and Help as the app's state has them now. */
export function useAppMenus(desktop: DesktopBridge | null = desktopFromWindow()): AppMenu[] {
  const { t } = useTranslation();
  const ready = useEngineReady();
  const known = useSessionStore((s) => s.known);
  const readOnly = useSessionStore((s) => s.readOnly !== null);
  const recovering = useSessionStore((s) => s.recovery !== null);
  const stacks = useSessionStore((s) => s.stacks);
  const busy = useSessionUiStore((s) => s.busy);
  const hasItems = useTimelineHasItems();
  const recent = useSessionUiStore((s) => s.recent);
  const dev = useDevBuild(desktop);
  const registered = useRegisteredPanels();
  const openPanels = useOpenPanels().open;
  // The shortcuts the menus show follow a rebinding at once (WP-2H.05).
  const preset = usePref("shortcuts.preset");
  const rebound = usePref("shortcuts.bindings");
  return useMemo(() => {
    const translate = translator(t);
    return buildAppMenu(
      {
        platform: currentMenuPlatform(),
        desktop: desktop !== null,
        dev,
        live: ready && known && !busy,
        readOnly,
        recovering,
        canUndo: stacks.undoDepth > 0,
        canRedo: stacks.redoDepth > 0,
        undoLabel: undoText(stacks, translate),
        redoLabel: redoText(stacks, translate),
        recent: recent.map((path) => ({ path, label: projectFileName(path) })),
        exportAvailable: hasItems,
        panels: registered.map((p) => ({
          id: p.id,
          label: t(p.title),
          checked: openPanels.includes(p.id),
        })),
      },
      translate,
    );
  }, [
    t,
    desktop,
    dev,
    ready,
    known,
    busy,
    readOnly,
    recovering,
    stacks,
    recent,
    hasItems,
    registered,
    openPanels,
    preset,
    rebound,
  ]);
}

/** The text-editing commands main runs on the editor window's focused element. */
const CLIPBOARD_COMMANDS: ReadonlySet<AppCommand> = new Set<AppCommand>([
  "cut",
  "copy",
  "paste",
  "selectAll",
]);

/** Project commands a shortcut may press while another project operation is still running. */
const DEFERRABLE: ReadonlySet<AppCommand> = new Set<AppCommand>(["new", "open", "save", "saveAs"]);

/** How long a deferred shortcut waits for the operation in flight before it is dropped. */
export const DEFER_SHORTCUT_MS = 10_000;

/**
 * The menu shortcuts, window-wide (not on macOS, whose native menu has them). A command runs
 * only while its menu item is enabled; `beforeRun` closes an open menu first.
 *
 * A project shortcut pressed while a project operation is running (Ctrl+N right after Ctrl+S,
 * say) runs once that operation has finished, as the native menu's accelerators did, instead
 * of being lost. Only the latest such key waits, for at most `DEFER_SHORTCUT_MS`.
 */
export function useAppShortcuts(
  menus: readonly AppMenu[],
  deps: AppCommandDeps,
  beforeRun: () => void = () => {},
): void {
  const latest = useRef({ menus, deps, beforeRun });
  latest.current = { menus, deps, beforeRun };
  const dev = menus.some((m) =>
    m.items.some((i) => i.kind === "command" && i.command === "reload"),
  );
  useEffect(() => {
    if (hasNativeMenu(latest.current.deps.desktop)) return;
    const platform = currentMenuPlatform();
    const onKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented) return;
      const command = commandForKey(event, platform, { typing: isTextEntry(event.target), dev });
      if (command === null) return;
      // Main cuts, copies and pastes in the editor window; in a detached panel's window the
      // browser does it there itself (the timeline hears it as a clipboard event).
      if (CLIPBOARD_COMMANDS.has(command) && !isMainWindow(windowOfEvent(event))) return;
      event.preventDefault();
      cancelDeferred();
      const now = latest.current;
      if (isEnabled(now.menus, command)) {
        now.beforeRun();
        runAppCommand(command, now.deps);
      } else if (DEFERRABLE.has(command) && sessionUiStore.getState().busy) {
        deferred = runWhenIdle(command);
      }
    };
    let deferred: (() => void) | null = null;
    const cancelDeferred = (): void => {
      deferred?.();
      deferred = null;
    };
    /** Runs `command` once nothing is busy and its menu item is enabled; returns a canceller. */
    const runWhenIdle = (command: AppCommand): (() => void) => {
      const deadline = Date.now() + DEFER_SHORTCUT_MS;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const check = (): void => {
        const now = latest.current;
        // The menus re-render after the operation ends, so poll briefly for the item.
        if (!sessionUiStore.getState().busy && isEnabled(now.menus, command)) {
          deferred = null;
          now.beforeRun();
          runAppCommand(command, now.deps);
        } else if (Date.now() < deadline) {
          timer = setTimeout(check, 25);
        } else {
          deferred = null;
        }
      };
      timer = setTimeout(check, 25);
      return () => clearTimeout(timer);
    };
    // Every editor window: Ctrl+Z in a detached panel's window undoes as it does here.
    const off = forEveryWindow((win) => {
      win.addEventListener("keydown", onKey);
      return () => win.removeEventListener("keydown", onKey);
    });
    return () => {
      off();
      cancelDeferred();
    };
  }, [dev]);
}
