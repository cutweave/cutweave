// SPDX-License-Identifier: GPL-3.0-or-later
// The title bar's save status (WP-1M.10): an icon whose tooltip says whether the project is
// saved. It replaces the Save button: saving is automatic (File > Settings… > Saving), and File >
// Save and Ctrl+S remain. Each state has its own icon, so it reads without colour.

import { type IconName, StatusChip, type StatusTone } from "@cutweave/ui";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { usePref } from "../prefs/prefsStore.ts";
import { useSessionStore } from "../session/sessionStore.ts";
import { useSessionUiStore } from "../session/sessionUiStore.ts";
import { useUnsavedWork } from "../session/useProject.ts";

export type SaveState =
  | "saved"
  | "unsavedAuto"
  | "unsavedManual"
  | "unsavedUntitled"
  | "saving"
  | "failed";

const STATES: Record<SaveState, { tone: StatusTone; icon: IconName; label: TKey }> = {
  saved: { tone: "success", icon: "circle-check", label: "saveStatus.saved" },
  unsavedAuto: { tone: "neutral", icon: "clock", label: "saveStatus.unsavedAuto" },
  unsavedManual: { tone: "warning", icon: "triangle-alert", label: "saveStatus.unsavedManual" },
  unsavedUntitled: {
    tone: "warning",
    icon: "triangle-alert",
    label: "saveStatus.unsavedUntitled",
  },
  saving: { tone: "neutral", icon: "loader-circle", label: "saveStatus.saving" },
  failed: { tone: "danger", icon: "circle-alert", label: "saveStatus.failed" },
};

/** What the title bar says about saving, from the session and the autosave preference. */
export function useSaveState(): SaveState {
  const unsaved = useUnsavedWork();
  const failed = useSessionStore((s) => s.autosaveFailed);
  const untitled = useSessionStore((s) => s.path === null);
  const saving = useSessionUiStore((s) => s.saving);
  const automatic = usePref("saving.autosave");
  if (saving) return "saving";
  if (!unsaved) return "saved";
  if (failed) return "failed";
  if (untitled) return "unsavedUntitled";
  return automatic ? "unsavedAuto" : "unsavedManual";
}

export function SaveStatusChip() {
  const { t } = useTranslation();
  const state = useSaveState();
  const unsaved = useUnsavedWork();
  const { tone, icon, label } = STATES[state];
  const chip = (
    <StatusChip tone={tone} icon={icon} iconOnly className="cw-save-status">
      {t(label)}
    </StatusChip>
  );
  return (
    <span className="cw-save-status__wrap" data-testid="save-status" data-state={state}>
      {unsaved ? (
        // The flows and the window title look for the unsaved marker by this id; it stays until
        // the save has finished, also while the status says "Saving…".
        <span className="cw-save-status__wrap" data-testid="dirty-dot">
          {chip}
        </span>
      ) : (
        chip
      )}
    </span>
  );
}
