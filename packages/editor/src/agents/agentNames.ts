// SPDX-License-Identifier: GPL-3.0-or-later
// How the editor names an agent (WP-1J.05): its actor (`agent:mcp`) with what the label means
// for the token kinds the engine hands out, so "Command line (agent:cli)" and not a bare label.

import type { TKey } from "../i18n/index.ts";
import type { Notice } from "../state/toastStore.ts";
import type { AgentNotice } from "./agentsStore.ts";

type Translate = (key: TKey, vars?: Record<string, unknown>) => string;

const KNOWN: Record<string, TKey> = {
  "agent:mcp": "agentUi.names.mcp",
  "agent:cli": "agentUi.names.cli",
};

/** "MCP client (agent:mcp)", "Command line (agent:cli)", or the actor itself. */
export function agentName(actor: string, t: Translate): string {
  const known = KNOWN[actor];
  return known === undefined ? actor : t("agentUi.named", { kind: t(known), actor });
}

/** The label after `agent:` (the token's), for the agent badge; the actor for anything else. */
export function agentLabel(actor: string): string {
  return actor.startsWith("agent:") ? actor.slice("agent:".length) : actor;
}

/** The toast of a notice. */
export function agentToast(notice: AgentNotice, t: Translate): Omit<Notice, "id"> {
  const name = agentName(notice.actor, t);
  return notice.kind === "connected"
    ? {
        tone: "info",
        title: t("agentUi.connectedTitle"),
        message: t("agentUi.connected", { name }),
      }
    : {
        tone: "info",
        title: t("agentUi.disconnectedTitle"),
        message: t("agentUi.disconnected", { name }),
      };
}
