// SPDX-License-Identifier: GPL-3.0-or-later
// What the editor says about its engine (WP-1L.03): the top bar's status chip, the "Engine
// restarted — recovered" notice after a supervised restart, and the blocking message while
// edits are refused (the engine failed for good, or it speaks another command schema).

import { Dialog, DialogNote, StatusChip, type StatusTone, Toast } from "@cutweave/ui";
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

const INDICATOR: Record<EngineIndicator, { tone: StatusTone; label: TKey }> = {
  none: { tone: "neutral", label: "engine.status.none" },
  connecting: { tone: "neutral", label: "engine.status.connecting" },
  ready: { tone: "success", label: "engine.status.ready" },
  reconnecting: { tone: "warning", label: "engine.status.reconnecting" },
  stopped: { tone: "danger", label: "engine.status.stopped" },
  mismatch: { tone: "danger", label: "engine.status.mismatch" },
};

/** The engine's state in the top bar: connected, reconnecting, stopped... */
export function EngineStatusChip() {
  const { t } = useTranslation();
  const { tone, label } = INDICATOR[useEngineStore(engineIndicator)];
  return (
    <StatusChip tone={tone} className="cw-engine-status">
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
