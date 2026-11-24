// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Settings > Agents and MCP and Settings > Notifications (WP-1J.05): the status line, the folders
// (main's picker adds them; the page never names one), the copyable client setups, the
// connected agents, and the notices that name an agent.

import { defaultPreferences, type PreferenceMap } from "@cutweave/preferences";
import type { AgentConnection } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { agentName, agentToast } from "../agents/agentNames.ts";
import {
  AgentNotices,
  agentsStore,
  createAgentsStore,
  type NoticeTimers,
} from "../agents/agentsStore.ts";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n, type TKey } from "../i18n/index.ts";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import type { AgentsBridge, PrefsBridge } from "../session/desktop.ts";
import { sessionStore } from "../session/sessionStore.ts";
import { SettingsDialog } from "../shell/SettingsDialog.tsx";
import { openSettings, settingsStore } from "./settingsStore.ts";

const t = (key: TKey, vars?: Record<string, unknown>) =>
  i18n.t(key as never, vars as never) as unknown as string;

const connection = (id: number, actor: string, extra: Partial<AgentConnection> = {}) =>
  ({
    id,
    actor,
    connected_at: 1_763_900_000_000,
    last_command: null,
    last_command_at: null,
    ...extra,
  }) as AgentConnection;

// ---- the notices -------------------------------------------------------------------------------

class FakeTimers implements NoticeTimers {
  pending = new Map<number, () => void>();
  #next = 1;
  set(fn: () => void): unknown {
    const id = this.#next++;
    this.pending.set(id, fn);
    return id;
  }
  clear(handle: unknown): void {
    this.pending.delete(handle as number);
  }
  fire(): void {
    const all = [...this.pending.values()];
    this.pending.clear();
    for (const fn of all) fn();
  }
}

describe("agent notices", () => {
  it("say once when an agent connects, and once when it has been gone for a moment", () => {
    const store = createAgentsStore();
    const timers = new FakeTimers();
    const seen: string[] = [];
    new AgentNotices({ notify: (n) => seen.push(`${n.kind} ${n.actor}`), timers }).watch(store);
    store.getState().opened(connection(1, "agent:mcp"));
    // A second connection of the same agent, and a quick reconnect (MCP's stateless revision,
    // a script running `cutweave exec` per step) are the same session.
    store.getState().opened(connection(2, "agent:mcp"));
    store.getState().closed(connection(1, "agent:mcp"));
    store.getState().closed(connection(2, "agent:mcp"));
    store.getState().opened(connection(3, "agent:mcp"));
    timers.fire();
    expect(seen).toEqual(["connected agent:mcp"]);
    store.getState().closed(connection(3, "agent:mcp"));
    expect(seen).toEqual(["connected agent:mcp"]);
    timers.fire();
    expect(seen).toEqual(["connected agent:mcp", "disconnected agent:mcp"]);
    // Another agent is another notice.
    store.getState().opened(connection(4, "agent:cli"));
    expect(seen.at(-1)).toBe("connected agent:cli");
  });

  it("take the connections open at a handshake as known, not news", () => {
    const store = createAgentsStore();
    const timers = new FakeTimers();
    const seen: string[] = [];
    new AgentNotices({ notify: (n) => seen.push(`${n.kind} ${n.actor}`), timers }).watch(store);
    store.getState().reset([connection(1, "agent:mcp")]);
    expect(seen).toEqual([]);
    store.getState().closed(connection(1, "agent:mcp"));
    timers.fire();
    expect(seen).toEqual(["disconnected agent:mcp"]);
    // A refresh only updates the last commands.
    store.getState().opened(connection(2, "agent:cli"));
    store
      .getState()
      .refresh([
        connection(2, "agent:cli", { last_command: "media.import", last_command_at: 5 }),
        connection(9, "agent:ghost"),
      ]);
    expect(store.getState().connections).toEqual([
      connection(2, "agent:cli", { last_command: "media.import", last_command_at: 5 }),
    ]);
  });

  it("name the agent", () => {
    expect(agentName("agent:cli", t)).toBe("Command line (agent:cli)");
    expect(agentName("agent:mcp", t)).toBe("MCP client (agent:mcp)");
    expect(agentName("agent:claude", t)).toBe("agent:claude");
    expect(agentToast({ kind: "connected", actor: "agent:cli" }, t)).toEqual({
      tone: "info",
      title: "Agent connected",
      message: "Command line (agent:cli) is connected to this window and can edit the project.",
    });
    expect(agentToast({ kind: "disconnected", actor: "agent:mcp" }, t).title).toBe(
      "Agent disconnected",
    );
  });
});

// ---- the page ---------------------------------------------------------------------------------

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView ??= () => {};
});

const SETUP = 'claude mcp add cutweave -- "C:\\Program Files\\Cutweave\\cutweave.exe" mcp --attach';

function fakeDesktop(initial: Partial<PreferenceMap> = {}, cliPath: string | null = "C:\\cw.exe") {
  const values: PreferenceMap = { ...defaultPreferences(), ...initial };
  const listeners = new Set<(changes: Partial<PreferenceMap>) => void>();
  const emit = (changes: Partial<PreferenceMap>) => {
    for (const listener of listeners) listener(changes);
  };
  const prefs: PrefsBridge = {
    get: async () => ({ values: { ...values }, defaults: defaultPreferences() }),
    set: async (key, value) => {
      (values as Record<string, unknown>)[key] = value;
      emit({ [key]: value } as Partial<PreferenceMap>);
    },
    reset: async () => {},
    exportToFile: async () => ({ status: "cancelled" as const }),
    importFromFile: async () => ({ status: "cancelled" as const }),
    onChanged: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  const store = (folders: string[]) => {
    values["agents.allowed_folders"] = folders;
    emit({ "agents.allowed_folders": folders });
  };
  const agents: AgentsBridge = {
    addFolder: vi.fn(async () => {
      store([...values["agents.allowed_folders"], "D:\\Footage"]);
      return { status: "added" as const, folder: "D:\\Footage" };
    }),
    removeFolder: vi.fn(async (folder: string) => {
      store(values["agents.allowed_folders"].filter((f) => f !== folder));
    }),
    connectInfo: vi.fn(async () => ({
      cliPath,
      setups:
        cliPath === null
          ? []
          : [
              { client: "claude-code" as const, text: SETUP },
              { client: "cursor" as const, text: '{"mcpServers":{}}' },
            ],
    })),
    copySetup: vi.fn(async () => SETUP),
  };
  return {
    desktop: { prefs, agents, dialog: { openProjectFolder: async () => null } },
    agents,
  };
}

let current: ReturnType<typeof fakeDesktop>;

async function install(desktop: ReturnType<typeof fakeDesktop>): Promise<void> {
  current = desktop;
  (globalThis as { cutweave?: unknown }).cutweave = desktop.desktop;
  await preferences.attach(desktop.desktop.prefs);
}

beforeEach(async () => {
  await install(fakeDesktop());
  engineStore.setState({ ...initialEngineState, connection: "ready" });
  sessionStore.setState({ path: "C:\\Films\\Trip\\Trip.cutweave" });
  agentsStore.setState({ connections: [] });
});

afterEach(async () => {
  cleanup();
  settingsStore.getState().setOpen(false);
  await preferences.attach(null);
  prefsStore.setState({ values: defaultPreferences() });
  engineStore.setState({ ...initialEngineState });
  sessionStore.setState({ path: null });
  (globalThis as { cutweave?: unknown }).cutweave = undefined;
});

const page = async () => {
  openSettings("agents");
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SettingsDialog))));
  const settings = await screen.findByRole("dialog", { name: "Settings" });
  await within(settings).findByRole("heading", { name: "Agents and MCP", level: 3 });
  return settings;
};

describe("Settings > Agents and MCP", () => {
  it("says agents can connect, or why not", async () => {
    const settings = await page();
    expect(await within(settings).findByText("Agents can connect to this window")).toBeTruthy();
    cleanup();
    engineStore.setState({ ...initialEngineState, connection: "reconnecting" });
    const down = await page();
    expect(
      await within(down).findByText("Agents cannot connect right now: the engine is not running"),
    ).toBeTruthy();
    cleanup();
    await install(fakeDesktop({}, null));
    engineStore.setState({ ...initialEngineState, connection: "ready" });
    const noCli = await page();
    expect(
      await within(noCli).findByText(
        "Agents cannot connect: this build has no cutweave command-line tool",
      ),
    ).toBeTruthy();
  });

  it("lists the open project's folder, which cannot be removed, then the added ones", async () => {
    const user = userEvent.setup();
    const settings = await page();
    const list = within(settings).getByRole("list", { name: "Folders agents can use" });
    expect(
      within(list).getByText("C:\\Films\\Trip (the open project, always allowed)"),
    ).toBeTruthy();
    expect(within(list).queryAllByRole("button")).toEqual([]);

    await user.click(within(settings).getByRole("button", { name: "Add folder…" }));
    expect(current.agents.addFolder).toHaveBeenCalledWith();
    expect(await within(settings).findByText("Agents can use D:\\Footage now.")).toBeTruthy();
    const remove = await within(settings).findByRole("button", { name: "Remove D:\\Footage" });
    await user.click(remove);
    expect(current.agents.removeFolder).toHaveBeenCalledWith("D:\\Footage");
    expect(await within(settings).findByText("Agents can no longer use D:\\Footage.")).toBeTruthy();
  });

  it("shows a client's setup and copies it through main", async () => {
    const user = userEvent.setup();
    const settings = await page();
    const setup = await within(settings).findByLabelText("Setup for Claude Code");
    expect(setup.textContent).toBe(SETUP);
    expect(setup.textContent).not.toMatch(/--pid|--port|token/);
    await user.click(within(settings).getByRole("button", { name: "Copy" }));
    expect(current.agents.copySetup).toHaveBeenCalledWith("claude-code");
    expect(await within(settings).findByText("Copied the Claude Code setup.")).toBeTruthy();
    await user.selectOptions(within(settings).getByRole("combobox", { name: "Client" }), "cursor");
    expect(within(settings).getByLabelText("Setup for Cursor").textContent).toBe(
      '{"mcpServers":{}}',
    );
  });

  it("re-reads the setups when the tool set changes", async () => {
    const user = userEvent.setup();
    const settings = await page();
    await within(settings).findByLabelText("Setup for Claude Code");
    const calls = vi.mocked(current.agents.connectInfo).mock.calls.length;
    await user.click(
      within(
        within(settings).getByRole("radiogroup", { name: "Tools in copied setups" }),
      ).getByRole("radio", { name: "Minimal" }),
    );
    await waitFor(() =>
      expect(vi.mocked(current.agents.connectInfo).mock.calls.length).toBeGreaterThan(calls),
    );
  });

  it("lists the connected agents with their last command", async () => {
    agentsStore.setState({
      connections: [
        connection(7, "agent:mcp", {
          last_command: "media.import",
          last_command_at: 1_763_900_001_000,
        }),
      ],
    });
    const settings = await page();
    const list = within(settings).getByRole("list", { name: "Connected agents" });
    expect(within(list).getByText("Agent: mcp")).toBeTruthy();
    expect(within(list).getByText(/^Last: media\.import at /)).toBeTruthy();
    agentsStore.setState({ connections: [] });
    expect(await within(settings).findByText("No agent is connected.")).toBeTruthy();
  });

  it("has no axe violations", async () => {
    agentsStore.setState({ connections: [connection(7, "agent:cli")] });
    const settings = await page();
    await within(settings).findByLabelText("Setup for Claude Code");
    for (const name of ["Agents and MCP", "Notifications"]) {
      await userEvent.setup().click(within(settings).getByRole("tab", { name }));
      const results = await axe.run(document.body, {
        rules: { "color-contrast": { enabled: false } },
      });
      expect(
        results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`),
        name,
      ).toEqual([]);
    }
  });
});

describe("Settings > Notifications", () => {
  it("turns the agent notices off", async () => {
    const user = userEvent.setup();
    openSettings("notifications");
    render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SettingsDialog))));
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    const toggle = within(settings).getByRole("switch", { name: "Agent connections" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await user.click(toggle);
    await waitFor(() => expect(prefsStore.getState().values["notify.agent_connected"]).toBe(false));
  });
});
