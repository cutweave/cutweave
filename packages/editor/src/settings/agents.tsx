// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Agents and MCP and Settings > Notifications (WP-1J.05). The page says whether agents
// can connect to this window, lists the folders they may use (the open project's, always, and the
// ones the person added with main's folder picker), copies a client setup that runs the bundled
// CLI (`cutweave mcp --attach`, no pid, port or token) and lists the agents connected now.
// Agents never change any of it: the folders reach the engine through main
// (`project.set_allowed_roots`, UI credential only), and preferences are main's.

import { CLIENT_PROFILES, type ClientProfile } from "@cutweave/preferences";
import { ActorBadge, Button, IconButton, StatusChip } from "@cutweave/ui";
import { useEffect, useState } from "react";
import { agentLabel } from "../agents/agentNames.ts";
import { agentsStore, useAgents } from "../agents/agentsStore.ts";
import { engineStore, useEngineReady } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import {
  AGENT_CLIENTS,
  type AgentClient,
  type AgentConnectInfo,
  type AgentFolderResult,
} from "../session/desktop.ts";
import { useSessionStore } from "../session/sessionStore.ts";
import {
  type ChoiceOption,
  registerSettingsPage,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";

/** How often the open page refreshes the connections' last commands. */
export const CONNECTIONS_REFRESH_MS = 2000;

/** The folder of a project document path (`D:\Films\Trip\Trip.cutweave` → `D:\Films\Trip`). */
export function projectFolder(documentPath: string | null): string | null {
  if (documentPath === null) return null;
  const cut = Math.max(documentPath.lastIndexOf("/"), documentPath.lastIndexOf("\\"));
  return cut > 0 ? documentPath.slice(0, cut) : null;
}

/** The rows of "Folders agents can use": the open project's folder (fixed), then the stored ones. */
export function folderItems(
  ctx: SettingsContext,
  documentPath: string | null,
): { id: string; label: string; fixed?: boolean }[] {
  const project = projectFolder(documentPath);
  const stored = ctx.values["agents.allowed_folders"].map((folder) => ({
    id: folder,
    label: folder,
  }));
  return project === null
    ? stored
    : [
        {
          id: "project",
          label: ctx.t("settings.rows.agentFolders.project", { path: project }),
          fixed: true,
        },
        ...stored,
      ];
}

/** What the page says after "Add folder…". */
export function addedMessage(ctx: SettingsContext, result: AgentFolderResult): string | null {
  switch (result.status) {
    case "added":
      return ctx.t("settings.rows.agentFolders.added", { folder: result.folder });
    case "exists":
      return ctx.t("settings.rows.agentFolders.exists", { folder: result.folder });
    case "full":
      return ctx.t("settings.rows.agentFolders.full");
    case "invalid":
      return ctx.t("settings.rows.agentFolders.invalid");
    case "cancelled":
      return null;
  }
}

async function addFolder(ctx: SettingsContext): Promise<void> {
  const agents = ctx.desktop?.agents;
  if (agents === undefined) return;
  const message = addedMessage(ctx, await agents.addFolder());
  if (message !== null) ctx.say(message);
}

async function removeFolder(ctx: SettingsContext, id: string): Promise<void> {
  const agents = ctx.desktop?.agents;
  if (agents === undefined || id === "project") return;
  await agents.removeFolder(id);
  ctx.say(ctx.t("settings.rows.agentFolders.removed", { folder: id }));
}

/** The bundled CLI and the setups main builds, read again when the tool set changes. */
function useConnectInfo(ctx: SettingsContext): AgentConnectInfo | null {
  const [info, setInfo] = useState<AgentConnectInfo | null>(null);
  const agents = ctx.desktop?.agents;
  const profile = ctx.values["agents.client_profile"];
  useEffect(() => {
    let live = true;
    // Main builds the setups with the stored tool set: read them again when it changes.
    void profile;
    if (agents === undefined) {
      setInfo({ cliPath: null, setups: [] });
      return;
    }
    agents
      .connectInfo()
      .then((next) => {
        if (live) setInfo(next);
      })
      .catch(() => {
        if (live) setInfo({ cliPath: null, setups: [] });
      });
    return () => {
      live = false;
    };
  }, [agents, profile]);
  return info;
}

/** "Agents can connect to this window", or why not. */
function AgentStatus({ ctx }: { ctx: SettingsContext }) {
  const engineReady = useEngineReady();
  const info = useConnectInfo(ctx);
  if (info === null) return null;
  const why = !engineReady
    ? ctx.t("settings.rows.agentStatus.noEngine")
    : info.cliPath === null
      ? ctx.t("settings.rows.agentStatus.noCli")
      : null;
  return (
    <div className="cw-agents__status" data-testid="agents-status">
      <StatusChip tone={why === null ? "success" : "warning"}>
        {why === null
          ? ctx.t("settings.rows.agentStatus.chipReady")
          : ctx.t("settings.rows.agentStatus.chipNot")}
      </StatusChip>
      <span className="caption">{why ?? ctx.t("settings.rows.agentStatus.ready")}</span>
      {why === null ? (
        <span className="caption">{ctx.t("settings.rows.agentStatus.readyHint")}</span>
      ) : null}
    </div>
  );
}

const CLIENT_NAMES: Record<AgentClient, TKey> = {
  "claude-code": "settings.rows.connectClient.clients.claude-code",
  "claude-desktop": "settings.rows.connectClient.clients.claude-desktop",
  cursor: "settings.rows.connectClient.clients.cursor",
  codex: "settings.rows.connectClient.clients.codex",
  stdio: "settings.rows.connectClient.clients.stdio",
};
const CLIENT_WHERE: Record<AgentClient, TKey> = {
  "claude-code": "settings.rows.connectClient.where.claude-code",
  "claude-desktop": "settings.rows.connectClient.where.claude-desktop",
  cursor: "settings.rows.connectClient.where.cursor",
  codex: "settings.rows.connectClient.where.codex",
  stdio: "settings.rows.connectClient.where.stdio",
};

/** A client's setup to copy: pick the client, read the setup, copy it. */
function ConnectClient({ ctx }: { ctx: SettingsContext }) {
  const info = useConnectInfo(ctx);
  const [client, setClient] = useState<AgentClient>("claude-code");
  if (info === null) return null;
  const setup = info.setups.find((s) => s.client === client);
  const clientName = (c: AgentClient) => ctx.t(CLIENT_NAMES[c]);
  if (setup === undefined) {
    return <span className="caption">{ctx.t("settings.rows.connectClient.none")}</span>;
  }
  const copy = async (): Promise<void> => {
    try {
      await ctx.desktop?.agents?.copySetup(client);
      ctx.say(ctx.t("settings.rows.connectClient.copied", { client: clientName(client) }));
    } catch {
      ctx.say(ctx.t("settings.rows.connectClient.copyFailed"));
    }
  };
  return (
    <div className="cw-agents__connect">
      <select
        className="cw-field body"
        aria-label={ctx.t("settings.rows.connectClient.clientLabel")}
        value={client}
        onChange={(event) => {
          const next = AGENT_CLIENTS.find((c) => c === event.target.value);
          if (next !== undefined) setClient(next);
        }}
      >
        {AGENT_CLIENTS.map((c) => (
          <option key={c} value={c}>
            {clientName(c)}
          </option>
        ))}
      </select>
      <figure
        className="cw-agents__setup"
        aria-label={ctx.t("settings.rows.connectClient.setupLabel", {
          client: clientName(client),
        })}
      >
        <pre className="cw-agents__code numeric">{setup.text}</pre>
      </figure>
      <span className="caption">{ctx.t(CLIENT_WHERE[client])}</span>
      <Button size="sm" icon="copy" onClick={() => void copy()}>
        {ctx.t("settings.rows.connectClient.copy")}
      </Button>
    </div>
  );
}

const clock = (ms: number): string => new Date(ms).toLocaleTimeString();

/** The agents connected now; the last commands refresh while the page is open. */
function ConnectedAgents({ ctx }: { ctx: SettingsContext }) {
  const connections = useAgents((s) => s.connections);
  useEffect(() => {
    const refresh = () => {
      const rpc = engineStore.getState().rpc;
      if (rpc === null || engineStore.getState().connection !== "ready") return;
      rpc
        .call("project.connections", {})
        .then((out) => agentsStore.getState().refresh(out.connections))
        .catch(() => undefined);
    };
    refresh();
    const id = window.setInterval(refresh, CONNECTIONS_REFRESH_MS);
    return () => window.clearInterval(id);
  }, []);
  if (connections.length === 0) {
    return <span className="caption">{ctx.t("settings.rows.connectedAgents.none")}</span>;
  }
  return (
    <ul
      className="cw-settings__items cw-agents__connected"
      aria-label={ctx.t("settings.rows.connectedAgents.listLabel")}
    >
      {connections.map((c) => (
        <li key={c.id} className="cw-agents__agent">
          <ActorBadge
            kind={c.actor.startsWith("plugin:") ? "plugin" : "agent"}
            id={agentLabel(c.actor)}
          />
          <span className="caption">
            {ctx.t("settings.rows.connectedAgents.since", { time: clock(c.connected_at) })}
          </span>
          <span className="caption">
            {c.last_command === null || c.last_command_at === null
              ? ctx.t("settings.rows.connectedAgents.lastNone")
              : ctx.t("settings.rows.connectedAgents.last", {
                  command: c.last_command,
                  time: clock(c.last_command_at),
                })}
          </span>
        </li>
      ))}
    </ul>
  );
}

/**
 * "Folders agents can use": the open project's folder (always allowed, not removable; it follows
 * the project, which is not a preference), then the folders the person added, each removable,
 * and "Add folder…", which opens main's folder picker.
 */
function FolderList({ ctx }: { ctx: SettingsContext }) {
  const documentPath = useSessionStore((s) => s.path);
  const items = folderItems(ctx, documentPath);
  const canChange = ctx.desktop?.agents !== undefined;
  const label = ctx.t("settings.rows.agentFolders.label");
  return (
    <div className="cw-settings__list">
      {items.length === 0 ? (
        <span className="caption">{ctx.t("settings.rows.agentFolders.empty")}</span>
      ) : (
        <ul className="cw-settings__items" aria-label={label}>
          {items.map((item) => (
            <li key={item.id} className="cw-settings__item body">
              <span className="cw-settings__itemlabel">{item.label}</span>
              {item.fixed ? null : (
                <IconButton
                  size="sm"
                  icon="x"
                  label={ctx.t("settings.rows.agentFolders.remove", { name: item.label })}
                  disabled={!canChange}
                  onClick={() => void removeFolder(ctx, item.id).catch(() => undefined)}
                />
              )}
            </li>
          ))}
        </ul>
      )}
      <Button
        size="sm"
        icon="folder-open"
        disabled={!canChange}
        onClick={() => void addFolder(ctx).catch(() => undefined)}
      >
        {ctx.t("settings.rows.agentFolders.add")}
      </Button>
    </div>
  );
}

const PROFILE_LABELS: Record<ClientProfile, TKey> = {
  minimal: "settings.rows.clientProfile.minimal",
  standard: "settings.rows.clientProfile.standard",
  full: "settings.rows.clientProfile.full",
};
const profileOptions: readonly ChoiceOption[] = CLIENT_PROFILES.map((profile) => ({
  value: profile,
  label: PROFILE_LABELS[profile],
}));

const agents: SettingsPageDef = {
  id: "agents",
  title: "settings.pages.agents",
  icon: "bot",
  group: "agents",
  order: 10,
  rows: [
    {
      id: "agents.status",
      label: "settings.rows.agentStatus.label",
      description: "settings.rows.agentStatus.description",
      keywords: "settings.rows.agentStatus.keywords",
      control: { kind: "custom", render: (ctx) => <AgentStatus ctx={ctx} /> },
    },
    {
      id: "agents.allowed_folders",
      key: "agents.allowed_folders",
      label: "settings.rows.agentFolders.label",
      description: "settings.rows.agentFolders.description",
      keywords: "settings.rows.agentFolders.keywords",
      control: { kind: "custom", render: (ctx) => <FolderList ctx={ctx} /> },
    },
    {
      id: "agents.client_profile",
      key: "agents.client_profile",
      label: "settings.rows.clientProfile.label",
      description: "settings.rows.clientProfile.description",
      keywords: "settings.rows.clientProfile.keywords",
      control: { kind: "segmented", options: profileOptions },
    },
    {
      id: "agents.connect",
      label: "settings.rows.connectClient.label",
      description: "settings.rows.connectClient.description",
      keywords: "settings.rows.connectClient.keywords",
      control: { kind: "custom", render: (ctx) => <ConnectClient ctx={ctx} /> },
    },
    {
      id: "agents.connected",
      label: "settings.rows.connectedAgents.label",
      description: "settings.rows.connectedAgents.description",
      keywords: "settings.rows.connectedAgents.keywords",
      control: { kind: "custom", render: (ctx) => <ConnectedAgents ctx={ctx} /> },
    },
  ],
};

const notifications: SettingsPageDef = {
  id: "notifications",
  title: "settings.pages.notifications",
  icon: "info",
  group: "preferences",
  order: 110,
  rows: [
    {
      id: "notify.agent_connected",
      key: "notify.agent_connected",
      label: "settings.rows.agentNotices.label",
      description: "settings.rows.agentNotices.description",
      keywords: "settings.rows.agentNotices.keywords",
      control: { kind: "switch" },
    },
    // Exports (WP-2G.04b): main shows the OS notification, the editor plays the sound.
    {
      id: "notify.export_done",
      key: "notify.export_done",
      label: "settings.rows.exportDone.label",
      description: "settings.rows.exportDone.description",
      keywords: "settings.rows.exportDone.keywords",
      control: { kind: "switch" },
    },
    {
      id: "notify.sound",
      key: "notify.sound",
      label: "settings.rows.exportSound.label",
      description: "settings.rows.exportSound.description",
      keywords: "settings.rows.exportSound.keywords",
      control: { kind: "switch" },
    },
  ],
};

export const AGENT_PAGES: readonly SettingsPageDef[] = [notifications, agents];

/** Registers Agents and MCP and Notifications (idempotent). */
export function registerAgentPages(): void {
  for (const page of AGENT_PAGES) registerSettingsPage(page);
}
registerAgentPages();
