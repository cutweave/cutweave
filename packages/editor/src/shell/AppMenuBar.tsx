// SPDX-License-Identifier: GPL-3.0-or-later
// File, Edit, View and Help in the title bar (WP-1M.09), drawn from the menu model (`appMenu.ts`)
// with `@cutweave/ui`'s `MenuBar`, and the shortcuts of its commands. Not shown on macOS, whose
// native app menu stays at the top of the screen (`TopBar` decides).
import { MenuBar, type MenuBarEntry, type MenuBarMenu } from "@cutweave/ui";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { desktopFromWindow } from "../session/desktop.ts";
import { projectController } from "../session/useProject.ts";
import { runWorkspaceCommand, TOGGLE_PANEL_PREFIX } from "../workspace/commands.ts";
import { type AppCommandDeps, runAppCommand, useAppMenus, useAppShortcuts } from "./appCommands.ts";
import type { AppMenu, AppMenuEntry } from "./appMenu.ts";

function toEntry(entry: AppMenuEntry, deps: AppCommandDeps): MenuBarEntry {
  switch (entry.kind) {
    case "separator":
      return { kind: "separator", id: entry.id };
    case "recent":
      return {
        kind: "submenu",
        id: "recent",
        label: entry.label,
        disabled: !entry.enabled,
        items:
          entry.items.length === 0
            ? [{ id: "none", label: entry.emptyLabel, disabled: true }]
            : entry.items.map((item) => ({
                id: item.path,
                label: item.label,
                icon: "folder-open",
                onSelect: () => void deps.controller.open(item.path),
              })),
      };
    case "panels":
      return {
        kind: "submenu",
        id: "panels",
        label: entry.label,
        disabled: !entry.enabled,
        items: entry.items.map((panel) => ({
          id: `panel-${panel.id}`,
          label: panel.label,
          checked: panel.checked,
          onSelect: () => void runWorkspaceCommand(`${TOGGLE_PANEL_PREFIX}${panel.id}`),
        })),
      };
    case "workspace":
      return {
        id: entry.command,
        label: entry.label,
        disabled: !entry.enabled,
        onSelect: () =>
          void runWorkspaceCommand(
            entry.command,
            entry.panel === undefined ? {} : { panel: entry.panel },
          ),
      };
    case "command":
      return {
        id: entry.command,
        label: entry.label,
        shortcut: entry.shortcut,
        disabled: !entry.enabled,
        onSelect: () => runAppCommand(entry.command, deps),
      };
  }
}

function toMenus(menus: readonly AppMenu[], deps: AppCommandDeps): MenuBarMenu[] {
  return menus.map((menu) => ({
    id: menu.id,
    label: menu.label,
    items: menu.items.map((entry) => toEntry(entry, deps)),
  }));
}

/** The application menu bar and its shortcuts. */
export function AppMenuBar() {
  const { t } = useTranslation();
  const desktop = desktopFromWindow();
  const deps = useMemo<AppCommandDeps>(
    () => ({ controller: projectController(), desktop }),
    [desktop],
  );
  const menus = useAppMenus(desktop);
  const [open, setOpen] = useState("");
  // A shortcut pressed while a menu is open closes it first, as a native menu does.
  useAppShortcuts(menus, deps, () => setOpen(""));
  return (
    <MenuBar
      aria-label={t("menu.barLabel")}
      className="cw-topbar__menubar"
      menus={toMenus(menus, deps)}
      value={open}
      onValueChange={setOpen}
    />
  );
}
