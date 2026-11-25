// SPDX-License-Identifier: GPL-3.0-or-later
import {
  Button,
  IconButton,
  type IconName,
  Menu,
  type MenuEntry,
  SegmentedControl,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import { ExportStatusChip } from "../export/ExportChip.tsx";
import { exportController, useTimelineHasItems } from "../export/useExport.ts";
import { useContextMenu } from "../menus/host.tsx";
import { hasNativeMenu } from "../session/desktop.ts";
import { redoText, undoText } from "../session/historyLabels.ts";
import { projectFileName, useSessionStore } from "../session/sessionStore.ts";
import { useSessionUiStore } from "../session/sessionUiStore.ts";
import { projectController } from "../session/useProject.ts";
import {
  useViewStore,
  viewStore,
  WORKSPACE_DEFS,
  WORKSPACES,
  type WorkspaceId,
} from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { runWorkspaceCommand } from "../workspace/commands.ts";
import { AppMenuBar } from "./AppMenuBar.tsx";
import { EngineStatusChip } from "./EngineNotices.tsx";
import { SaveStatusChip } from "./SaveStatus.tsx";

/** The workspace switcher shows icons (every pixel of the title bar counts); names are tooltips. */
const WORKSPACE_ICONS: Record<WorkspaceId, IconName> = {
  "short-form": "smartphone",
  "long-form": "monitor",
};

/** The modifier key cap: ⌘ on macOS, Ctrl elsewhere. */
function modifierKey(): string {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  return /mac/i.test(platform) ? "⌘" : "Ctrl";
}

/** Recent projects as a dropdown (macOS only: its native menu has no Open recent). */
function RecentMenu({ disabled }: { disabled: boolean }) {
  const { t } = useTranslation();
  const recent = useSessionUiStore((s) => s.recent);
  const controller = projectController();
  const items: MenuEntry[] =
    recent.length === 0
      ? [{ id: "none", label: t("topbar.recentEmpty"), disabled: true }]
      : recent.map((path) => ({
          id: path,
          label: projectFileName(path),
          icon: "folder-open",
          onSelect: () => void controller.open(path),
        }));
  return (
    <Menu mode="dropdown" aria-label={t("topbar.recent")} items={items}>
      <IconButton icon="chevron-down" size="sm" label={t("topbar.recent")} disabled={disabled} />
    </Menu>
  );
}

/**
 * Top bar, which is also the window's title bar (WP-1M.09): the brand and the application menu
 * bar (File, Edit, View, Help; macOS keeps its native menu instead), undo/redo (WP-1M.07;
 * disabled until the engine connects), the workspace switcher, and the export, save and engine
 * status with Export at the right (WP-1M.08, WP-1M.10). Saving is automatic and has no button
 * here: File > Save and Ctrl+S remain, and the save status says whether everything is saved.
 * Every pixel here counts: New, Open, Save as and the theme are in the menus and Settings. Its
 * empty areas drag the window and a double-click maximizes it; the OS draws the window controls
 * over its right end (left on macOS), which the layout leaves free.
 */
export function TopBar() {
  const { t } = useTranslation();
  const workspaceId = useViewStore((s) => s.workspaceId);
  const ready = useEngineReady();
  const known = useSessionStore((s) => s.known);
  const readOnly = useSessionStore((s) => s.readOnly !== null);
  const recovering = useSessionStore((s) => s.recovery !== null);
  const stacks = useSessionStore((s) => s.stacks);
  const busy = useSessionUiStore((s) => s.busy);
  const hasItems = useTimelineHasItems();
  const controller = projectController();
  const live = ready && known && !busy;
  const mod = modifierKey();
  const translate = translator(t);
  const undoName = undoText(stacks, translate);
  const redoName = redoText(stacks, translate);
  // Right-click on either button: Undo, Redo and Open History… (WP-2H.11).
  const undoMenu = useContextMenu<HTMLButtonElement>("topbar.history", () => ({
    ids: [],
    name: undoName,
  }));
  const redoMenu = useContextMenu<HTMLButtonElement>("topbar.history", () => ({
    ids: [],
    name: redoName,
  }));
  return (
    <header className="cw-topbar">
      <div className="cw-topbar__group">
        <span className="cw-topbar__brand heading">{t("app.name")}</span>
        {hasNativeMenu() ? null : <AppMenuBar />}
        <Toolbar aria-label={t("topbar.toolbarLabel")} className="cw-topbar__actions">
          {/* New, Open, Open recent and Save as live in the File menu with their shortcuts. macOS
              keeps the native menu, which has no Open recent, so its list stays here. */}
          {hasNativeMenu() ? (
            <>
              <RecentMenu disabled={!live} />
              <ToolbarSeparator />
            </>
          ) : null}
          <Tooltip label={undoName} keys={[mod, "Z"]}>
            <IconButton
              ref={undoMenu}
              icon="undo-2"
              label={undoName}
              disabled={!live || readOnly || stacks.undoDepth === 0}
              onClick={() => void controller.undo()}
            />
          </Tooltip>
          <Tooltip label={redoName} keys={[mod, "Shift", "Z"]}>
            <IconButton
              ref={redoMenu}
              icon="redo-2"
              label={redoName}
              disabled={!live || readOnly || stacks.redoDepth === 0}
              onClick={() => void controller.redo()}
            />
          </Tooltip>
        </Toolbar>
      </div>
      <div className="cw-topbar__group cw-topbar__group--center">
        <SegmentedControl<WorkspaceId>
          aria-label={t("topbar.workspaceLabel")}
          value={workspaceId}
          onChange={(id) => viewStore.getState().switchWorkspace(id)}
          iconOnly
          options={WORKSPACES.map((id) => ({
            value: id,
            label: t(WORKSPACE_DEFS[id].labelKey),
            icon: WORKSPACE_ICONS[id],
          }))}
        />
        <Tooltip label={t("workspace.reset")}>
          <IconButton
            icon="refresh-cw"
            label={t("workspace.reset")}
            size="sm"
            onClick={() => runWorkspaceCommand("workspace.reset_layout")}
          />
        </Tooltip>
      </div>
      <div className="cw-topbar__group cw-topbar__group--end">
        <ExportStatusChip />
        <SaveStatusChip />
        <EngineStatusChip />
        <Tooltip label={t("topbar.export")} keys={[mod, "M"]}>
          <Button
            variant="primary"
            icon="upload"
            disabled={!live || recovering || !hasItems}
            onClick={() => exportController().open()}
          >
            {t("topbar.export")}
          </Button>
        </Tooltip>
      </div>
    </header>
  );
}
