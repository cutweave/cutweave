// SPDX-License-Identifier: GPL-3.0-or-later
// What the editor says about its engine (WP-1L.03): the title bar's status icon, the "Engine
// restarted — recovered" notice after a supervised restart, and the blocking message while
// edits are refused (the engine failed for good, or it speaks another command schema).

import {
  Dialog,
  DialogNote,
  type IconName,
  StatusChip,
  type StatusTone,
  Toast,
} from "@cutweave/ui";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  type EngineIndicator,
  type EngineNotice,
  engineIndicator,
  useEngineStore,
} from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";

/** How long the restart notice stays when everything was recovered. */
export const RESTART_NOTICE_MS = 3000;

/** Each state has its own icon, so the icon-only chip reads without colour (design README). */
const INDICATOR: Record<EngineIndicator, { tone: StatusTone; label: TKey; icon: IconName }> = {
  none: { tone: "neutral", label: "engine.status.none", icon: "plug" },
  connecting: { tone: "neutral", label: "engine.status.connecting", icon: "loader-circle" },
  ready: { tone: "success", label: "engine.status.ready", icon: "check" },
  reconnecting: { tone: "warning", label: "engine.status.reconnecting", icon: "loader-circle" },
  stopped: { tone: "danger", label: "engine.status.stopped", icon: "circle-alert" },
  mismatch: { tone: "danger", label: "engine.status.mismatch", icon: "unlink" },
};

/**
 * The engine's state in the title bar (connected, reconnecting, stopped...) as an icon; the
 * words are its tooltip and its text for screen readers.
 */
export function EngineStatusChip() {
  const { t } = useTranslation();
  const { tone, label, icon } = INDICATOR[useEngineStore(engineIndicator)];
  return (
    <StatusChip tone={tone} icon={icon} iconOnly className="cw-engine-status">
      {t(label)}
    </StatusChip>
  );
}

/** The file name of a project path, for messages ("film.cutweave"). */
function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function RestartNotice({ notice }: { notice: EngineNotice }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const recovery = notice.recovery;
  if (recovery === null) {
    // Nothing was journaled: say so, and keep it until it is read.
    return (
      <Toast
        open={open}
        onOpenChange={setOpen}
        tone="warning"
        title={t("engine.restarted.untitledTitle")}
        message={t("engine.restarted.untitled")}
      />
    );
  }
  const project = projectName(recovery.project);
  const restored =
    recovery.applied === 0
      ? t("engine.restarted.nothingToRestore", { project })
      : recovery.applied === 1
        ? t("engine.restarted.appliedOne", { project })
        : t("engine.restarted.appliedMany", { count: recovery.applied, project });
  return (
    <Toast
      open={open}
      onOpenChange={setOpen}
      tone="success"
      duration={RESTART_NOTICE_MS}
      title={t("engine.restarted.recoveredTitle")}
      message={recovery.damaged ? `${restored} ${t("engine.restarted.damaged")}` : restored}
    />
  );
}

/** Blocks the editor while edits are refused; there is nothing to dismiss until it is resolved. */
function EditBlocker() {
  const { t } = useTranslation();
  const blocked = useEngineStore((s) => s.blocked);
  const mismatch = blocked?.kind === "schema-mismatch";
  return (
    <Dialog
      open={blocked !== null}
      onOpenChange={() => {}}
      dirty
      title={t(mismatch ? "engine.blocked.mismatchTitle" : "engine.blocked.failedTitle")}
    >
      <DialogNote icon="circle-alert">
        {mismatch
          ? t("engine.blocked.mismatchBody", { version: blocked.engineVersion })
          : t("engine.blocked.failedBody")}
      </DialogNote>
    </Dialog>
  );
}

/** The restart notice and the edit blocker; mount once inside a `ToastProvider`. */
export function EngineNotices() {
  const notice = useEngineStore((s) => s.notice);
  return (
    <>
      {/* A new notice replaces the previous one (keyed by id, so it opens again). */}
      {notice === null ? null : <RestartNotice key={notice.id} notice={notice} />}
      <EditBlocker />
    </>
  );
}
