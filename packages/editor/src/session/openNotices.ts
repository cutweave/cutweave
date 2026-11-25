// SPDX-License-Identifier: GPL-3.0-or-later
// What the person is told when an agent or a plugin opens a project (ADR-0029): a toast in agent
// colour after it opened, and one that waits for them when the open was refused because of their
// unsaved changes or failed.

import { agentName } from "../agents/agentNames.ts";
import type { TKey } from "../i18n/index.ts";
import type { Notice } from "../state/toastStore.ts";
import { projectNameOfPath } from "./sessionStore.ts";
import type { OpenNotice } from "./sessionSync.ts";

type Translate = (key: TKey, vars?: Record<string, unknown>) => string;

/** The toast of an open notice. */
export function openNoticeToast(notice: OpenNotice, t: Translate): Omit<Notice, "id"> {
  const agent = agentName(notice.actor, t);
  if (notice.kind === "opened") {
    const name = notice.name === "" ? projectNameOfPath(notice.path) : notice.name;
    return {
      tone: "agent",
      title: t("projectUi.agentOpen.openedTitle"),
      message: t("projectUi.agentOpen.opened", { agent, name }),
    };
  }
  const name = projectNameOfPath(notice.path);
  const current = notice.current === "" ? t("projectUi.untitled") : notice.current;
  return {
    tone: "agent",
    title: t("projectUi.failed.open"),
    message:
      notice.kind === "refused"
        ? t("projectUi.agentOpen.refused", { agent, name, current })
        : t("projectUi.agentOpen.failed", { agent, name, message: notice.message }),
    // The person has something to do (save, or look into it): it waits for them.
    duration: Number.POSITIVE_INFINITY,
  };
}
