// SPDX-License-Identifier: GPL-3.0-or-later
// The `workspace.*` view commands (WP-2H.01, UI-only: they never touch the project document, so
// they are not in the engine's command catalog and not undoable). Every panel action a person
// can take goes through here: the tab's close button and menu, View > Panels, Reset layout, the
// empty state. The command palette (WP-2H.04) lists `listWorkspaceCommands()`; the keymap
// (WP-2H.05) binds their ids. Each panel also has its own toggle command (`workspace.toggle_panel.<id>`)
// so a key or a palette entry can name one panel without arguments.
import type { TKey } from "../i18n/index.ts";
import { viewStore, WORKSPACES, type WorkspaceId } from "../state/viewStore.ts";
import { closePanel, openPanel, reopenAllPanels, togglePanel } from "./dockHost.ts";
import { type PanelId, registeredPanels } from "./panelRegistry.ts";

export interface WorkspaceCommand {
  /** `workspace.<verb>`; panel toggles are `workspace.toggle_panel.<panel id>`. */
  readonly id: string;
  /** Catalog key of its name (palette, shortcuts page). A panel toggle uses `panelCommand.toggle`. */
  readonly title: TKey;
  /** The panel it is about, for the per-panel toggles. */
  readonly panel?: PanelId;
}

export interface WorkspaceCommandArgs {
  readonly panel?: PanelId;
  readonly workspace?: WorkspaceId;
}

const BASE: readonly WorkspaceCommand[] = [
  { id: "workspace.open_panel", title: "workspace.commands.openPanel" },
  { id: "workspace.close_panel", title: "workspace.commands.closePanel" },
  { id: "workspace.toggle_panel", title: "workspace.commands.togglePanel" },
  { id: "workspace.reopen_panels", title: "workspace.commands.reopenPanels" },
  { id: "workspace.reset_layout", title: "workspace.reset" },
  { id: "workspace.reset_all", title: "workspace.resetAll" },
  { id: "workspace.switch_workspace", title: "workspace.commands.switch" },
];

export const TOGGLE_PANEL_PREFIX = "workspace.toggle_panel.";

/** Every workspace command, including one toggle per registered panel. */
export function listWorkspaceCommands(): readonly WorkspaceCommand[] {
  return [
    ...BASE,
    ...registeredPanels().map(
      (p): WorkspaceCommand => ({
        id: `${TOGGLE_PANEL_PREFIX}${p.id}`,
        title: "workspace.commands.togglePanelNamed",
        panel: p.id,
      }),
    ),
  ];
}

/** Resets the workspace being shown to its default layout, every panel open. */
export function resetCurrentWorkspace(): void {
  const view = viewStore.getState();
  view.resetWorkspace(view.workspaceId);
}

/** Resets every workspace to its default layout. */
export function resetAllWorkspaces(): void {
  const view = viewStore.getState();
  for (const id of WORKSPACES) view.resetWorkspace(id);
}

/** Runs a workspace command; `false` when the id is unknown or its argument is missing. */
export function runWorkspaceCommand(id: string, args: WorkspaceCommandArgs = {}): boolean {
  if (id.startsWith(TOGGLE_PANEL_PREFIX)) {
    togglePanel(id.slice(TOGGLE_PANEL_PREFIX.length));
    return true;
  }
  switch (id) {
    case "workspace.open_panel":
    case "workspace.close_panel":
    case "workspace.toggle_panel": {
      if (args.panel === undefined) return false;
      if (id === "workspace.open_panel") openPanel(args.panel);
      else if (id === "workspace.close_panel") closePanel(args.panel);
      else togglePanel(args.panel);
      return true;
    }
    case "workspace.reopen_panels":
      reopenAllPanels();
      return true;
    case "workspace.reset_layout":
      resetCurrentWorkspace();
      return true;
    case "workspace.reset_all":
      resetAllWorkspaces();
      return true;
    case "workspace.switch_workspace":
      if (args.workspace === undefined) return false;
      viewStore.getState().switchWorkspace(args.workspace);
      return true;
    default:
      return false;
  }
}
