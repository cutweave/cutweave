// SPDX-License-Identifier: GPL-3.0-or-later
import {
  Button,
  IconButton,
  Menu,
  type MenuEntry,
  SegmentedControl,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import { useEngineReady } from "../engine/engineStore.ts";
import { historyLabel } from "../session/historyLabels.ts";
import { projectFileName, useSessionStore } from "../session/sessionStore.ts";
import { useSessionUiStore } from "../session/sessionUiStore.ts";
import { projectController, useUnsavedWork } from "../session/useProject.ts";
import {
  THEME_LABEL_KEYS,
  THEMES,
  type ThemeId,
  useViewStore,
  viewStore,
  WORKSPACE_DEFS,
  WORKSPACES,
  type WorkspaceId,
} from "../state/viewStore.ts";
import { translator } from "../timeline/errors.ts";
import { EngineStatusChip } from "./EngineNotices.tsx";

/** The modifier key cap: ⌘ on macOS, Ctrl elsewhere. */
function modifierKey(): string {
  const platform = typeof navigator === "undefined" ? "" : navigator.platform;
  return /mac/i.test(platform) ? "⌘" : "Ctrl";
}

/** Recent projects as a dropdown next to Open. */
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
 * Top bar: project actions and undo/redo (WP-1M.07; disabled until the engine connects), the
 * workspace switcher and the theme switch. The brand sits left, export right.
 */
export function TopBar() {
  const { t } = useTranslation();
  const workspaceId = useViewStore((s) => s.workspaceId);
  const theme = useViewStore((s) => s.theme);
  const ready = useEngineReady();
  const known = useSessionStore((s) => s.known);
  const dirty = useUnsavedWork();
  const readOnly = useSessionStore((s) => s.readOnly !== null);
  const recovering = useSessionStore((s) => s.recovery !== null);
  const stacks = useSessionStore((s) => s.stacks);
  const busy = useSessionUiStore((s) => s.busy);
  const controller = projectController();
  const live = ready && known && !busy;
  const mod = modifierKey();
  const translate = translator(t);
  const undoName =
    stacks.nextUndo === null
      ? t("topbar.undo")
      : t("topbar.undoAction", { label: historyLabel(stacks.nextUndo, translate) });
  const redoName =
    stacks.nextRedo === null
      ? t("topbar.redo")
      : t("topbar.redoAction", { label: historyLabel(stacks.nextRedo, translate) });
  return (
    <header className="cw-topbar">
      <div className="cw-topbar__group">
        <span className="cw-topbar__brand heading">{t("app.name")}</span>
        <Toolbar aria-label={t("topbar.toolbarLabel")} className="cw-topbar__actions">
          <Tooltip label={t("topbar.new")} keys={[mod, "N"]}>
            <Button
              variant="ghost"
              icon="plus"
              disabled={!live}
              onClick={() => void controller.newProject()}
            >
              {t("topbar.new")}
            </Button>
          </Tooltip>
          <Tooltip label={t("topbar.open")} keys={[mod, "O"]}>
            <Button
              variant="ghost"
              icon="folder-open"
              disabled={!live}
              onClick={() => void controller.open()}
            >
              {t("topbar.open")}
            </Button>
          </Tooltip>
          <RecentMenu disabled={!live} />
          <Tooltip label={t("topbar.save")} keys={[mod, "S"]}>
            <Button
              variant="ghost"
              disabled={!live || readOnly || recovering}
              onClick={() => void controller.save()}
            >
              {t("topbar.save")}
              {dirty ? (
                <span
                  className="cw-topbar__dirty"
                  role="img"
                  aria-label={t("topbar.unsaved")}
                  data-testid="dirty-dot"
                />
              ) : null}
            </Button>
          </Tooltip>
          <Tooltip label={t("topbar.saveAs")} keys={[mod, "Shift", "S"]}>
            <IconButton
              icon="copy"
              size="sm"
              label={t("topbar.saveAs")}
              disabled={!live || recovering}
              onClick={() => void controller.saveAs()}
            />
          </Tooltip>
          <ToolbarSeparator />
          <Tooltip label={undoName} keys={[mod, "Z"]}>
            <IconButton
              icon="undo-2"
              label={undoName}
              disabled={!live || readOnly || stacks.undoDepth === 0}
              onClick={() => void controller.undo()}
            />
          </Tooltip>
          <Tooltip label={redoName} keys={[mod, "Shift", "Z"]}>
            <IconButton
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
          options={WORKSPACES.map((id) => ({ value: id, label: t(WORKSPACE_DEFS[id].labelKey) }))}
        />
        <Tooltip label={t("workspace.reset")}>
          <IconButton
            icon="refresh-cw"
            label={t("workspace.reset")}
            size="sm"
            onClick={() => viewStore.getState().resetWorkspace(workspaceId)}
          />
        </Tooltip>
      </div>
      <div className="cw-topbar__group cw-topbar__group--end">
        <EngineStatusChip />
        <SegmentedControl<ThemeId>
          aria-label={t("topbar.themeLabel")}
          value={theme}
          onChange={(id) => viewStore.getState().setTheme(id)}
          options={THEMES.map((id) => ({ value: id, label: t(THEME_LABEL_KEYS[id]) }))}
        />
        <Button variant="primary" icon="upload" disabled>
          {t("topbar.export")}
        </Button>
      </div>
    </header>
  );
}
