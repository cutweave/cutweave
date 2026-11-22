// SPDX-License-Identifier: GPL-3.0-or-later
import {
  Button,
  IconButton,
  SegmentedControl,
  StatusChip,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import { useTranslation } from "react-i18next";
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

/**
 * Top bar: project actions (disabled until the engine connects, WP-1M.02), the workspace
 * switcher and the theme switch. The brand sits left, export right.
 */
export function TopBar() {
  const { t } = useTranslation();
  const workspaceId = useViewStore((s) => s.workspaceId);
  const theme = useViewStore((s) => s.theme);
  return (
    <header className="cw-topbar">
      <div className="cw-topbar__group">
        <span className="cw-topbar__brand heading">{t("app.name")}</span>
        <Toolbar aria-label={t("topbar.toolbarLabel")} className="cw-topbar__actions">
          <Button variant="ghost" icon="folder-open" disabled>
            {t("topbar.open")}
          </Button>
          <Button variant="ghost" disabled>
            {t("topbar.save")}
          </Button>
          <ToolbarSeparator />
          <Tooltip label={t("topbar.undo")}>
            <IconButton icon="undo-2" label={t("topbar.undo")} disabled />
          </Tooltip>
          <Tooltip label={t("topbar.redo")}>
            <IconButton icon="redo-2" label={t("topbar.redo")} disabled />
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
        <StatusChip tone="neutral">{t("topbar.engineOffline")}</StatusChip>
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
