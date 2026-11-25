// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.13 acceptance (Vitest part): the surface tables of today's panels. A menu choice names the
// command the CLI and MCP have, with arguments that fit its schema (the parity of the menu with
// them is that the call is the same); the clip menu names a selection ("Delete 3 clips") and says
// why a row cannot run ("Track is locked"); the target rules of the bin and the timeline; the
// walk over every table; axe on every opened menu.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Project } from "@cutweave/protocol";
import type { MenuEntry, MenuItem, MenuSubmenu } from "@cutweave/ui";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../../engine/engineStore.ts";
import { i18n } from "../../i18n/index.ts";
import { sectionsStore } from "../../inspector/sections.ts";
import { binStore } from "../../media/binStore.ts";
import { currentOpenFolder, folderUiStore } from "../../media/folderUi.ts";
import { layoutStore } from "../../media/layoutStore.ts";
import { docStore } from "../../state/docStore.ts";
import { toastStore } from "../../state/toastStore.ts";
import { initialViewState, viewStore } from "../../state/viewStore.ts";
import { buildMenu } from "../build.ts";
import { registerBuiltinMenus } from "../builtins.ts";
import { menuDialogStore } from "../dialogs.ts";
import { ContextMenuHost, closeContextMenu, openContextMenu } from "../host.tsx";
import { createMenuServices, type MenuServices } from "../services.ts";
import { applyTargetRules } from "../target.ts";
import type { MenuTarget } from "../types.ts";
import { registerSurfaceMenus, SURFACES } from "./index.ts";

const F = 705_600_000;
const hex = (prefix: string, n: number) => `${prefix}_${n.toString(16).padStart(32, "0")}`;

const SEQ = hex("seq", 1);
const V1 = hex("trk", 1);
const A1 = hex("trk", 2);
const V2 = hex("trk", 3);
const LOCKED = hex("trk", 4);
const C1 = hex("itm", 1);
const C2 = hex("itm", 2);
const C3 = hex("itm", 3);
const CL = hex("itm", 4);
const MED_V = hex("med", 1);
const MED_A = hex("med", 2);
const MED_U = hex("med", 3);

const clip = (id: string, media: string, start: number, extra: object = {}) => ({
  kind: "clip",
  id,
  name: null,
  start: start * F,
  duration: 10 * F,
  enabled: true,
  linked_group: null,
  media,
  source_in: 0,
  audio: { gain: 0 },
  visual: { transform: {}, opacity: 1 },
  custom: {},
  ...extra,
});

const track = (id: string, kind: string, items: object[] = [], extra: object = {}) => ({
  id,
  kind,
  name: `Track ${id.slice(-2)}`,
  items: Object.fromEntries(items.map((i) => [(i as { id: string }).id, i])),
  muted: false,
  hidden: false,
  locked: false,
  solo: false,
  magnetic: false,
  gain: 0,
  custom: {},
  ...extra,
});

const asset = (id: string, kind: string, name: string, seconds: number) => ({
  id,
  kind,
  path_abs: `/media/${name}`,
  path_rel: null,
  probe: {
    streams: [{ index: 0, kind, codec: "x" }],
    duration: seconds * F,
    fps: kind === "video" ? { num: 30, den: 1 } : null,
  },
});

const project = (): Project =>
  ({
    meta: { name: "P" },
    settings: {},
    media: {
      [MED_V]: asset(MED_V, "video", "interview.mp4", 60),
      [MED_A]: asset(MED_A, "audio", "music.wav", 90),
      [MED_U]: asset(MED_U, "video", "spare.mp4", 5),
    },
    sequences: {
      [SEQ]: {
        id: SEQ,
        name: "Main",
        format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
        track_order: [V2, V1, A1, LOCKED],
        tracks: {
          [V1]: track(V1, "video", [
            clip(C1, MED_V, 0),
            clip(C2, MED_V, 10),
            clip(C3, MED_V, 20, { enabled: false }),
          ]),
          [V2]: track(V2, "video"),
          [A1]: track(A1, "audio", [], { muted: true }),
          [LOCKED]: track(LOCKED, "video", [clip(CL, MED_V, 0)], { locked: true }),
        },
        custom: {},
      },
    },
    custom: {},
  }) as unknown as Project;

const catalog = JSON.parse(
  readFileSync(join(import.meta.dirname, "../../../../../schema/commands.json"), "utf8"),
) as { commands: { name: string; input_schema: Schema }[] };
const schemaOf = new Map(catalog.commands.map((c) => [c.name, c.input_schema]));

interface Schema {
  readonly properties?: Record<string, { pattern?: string; items?: { pattern?: string } }>;
  readonly required?: readonly string[];
  readonly additionalProperties?: boolean;
}

/** A light check of arguments against a command's input schema: names, required, id patterns. */
function schemaProblems(command: string, args: unknown): string[] {
  const schema = schemaOf.get(command);
  if (schema === undefined) return [`${command} is not in the catalog`];
  const value = (args ?? {}) as Record<string, unknown>;
  const problems: string[] = [];
  for (const key of schema.required ?? []) {
    if (value[key] === undefined) problems.push(`${command}: missing ${key}`);
  }
  for (const [key, given] of Object.entries(value)) {
    const spec = schema.properties?.[key];
    if (spec === undefined) {
      if (schema.additionalProperties === false) problems.push(`${command}: unknown ${key}`);
      continue;
    }
    const check = (text: unknown, pattern: string | undefined) => {
      if (pattern !== undefined && (typeof text !== "string" || !new RegExp(pattern).test(text))) {
        problems.push(`${command}: ${key} does not match ${pattern}`);
      }
    };
    if (Array.isArray(given)) for (const item of given) check(item, spec.items?.pattern);
    else check(given, spec.pattern);
  }
  return problems;
}

let call: ReturnType<typeof vi.fn<(name: string, args?: unknown) => Promise<unknown>>>;
let services: MenuServices;

function setup(): void {
  call = vi.fn(async () => ({}));
  services = createMenuServices({
    call,
    keysFor: () => undefined,
    baseKeys: () => ({ "engine.live": true, "project.readOnly": false }),
    playhead: () => viewStore.getState().playhead,
    t: (spec) => i18n.t(spec.key, spec.vars ?? {}) as string,
    now: () => 0,
    report: vi.fn(),
  });
  registerBuiltinMenus(services);
  registerSurfaceMenus(services);
}

const allItems = (entries: readonly MenuEntry[]): MenuItem[] =>
  entries.flatMap((e) =>
    e.kind === "submenu"
      ? allItems((e as MenuSubmenu).items)
      : e.kind === undefined || e.kind === "item"
        ? [e as MenuItem]
        : [],
  );

/** Opens the menu of `surface` as the host would, with the target rules applied. */
function menuFor(surface: string, target: Partial<MenuTarget> = {}) {
  const def = services.surfaces.get(surface);
  if (def === undefined) throw new Error(`no surface ${surface}`);
  let chosen: (() => void | Promise<void>) | null = null;
  const resolved = applyTargetRules({ ids: [], ...target, surface }, def.selection);
  const menu = buildMenu(services, resolved, (run) => {
    chosen = run;
  });
  if (menu === null) throw new Error(`${surface} built no menu`);
  const items = allItems(menu.entries);
  return {
    menu,
    items,
    item: (label: RegExp | string) => {
      const found = items.find((i) =>
        typeof label === "string" ? i.label === label : label.test(i.label),
      );
      if (found === undefined)
        throw new Error(`no row ${String(label)} in ${items.map((i) => i.label).join(" | ")}`);
      return found;
    },
    /** Chooses the row and runs what the host would run after the menu closed. */
    async choose(label: RegExp | string) {
      chosen = null;
      this.item(label).onSelect?.();
      await (chosen as (() => void | Promise<void>) | null)?.();
    },
  };
}

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

beforeEach(() => {
  setup();
  docStore.setState({ status: "ready", rev: 1, committedRev: 1, project: project() });
  viewStore.setState({ ...initialViewState, playhead: 5 * F });
  toastStore.getState().clear();
  menuDialogStore.getState().close();
});

afterEach(() => {
  cleanup();
  closeContextMenu();
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
});

describe("the clip menu", () => {
  it("splits at the playhead when it is inside the clip, with the catalog's arguments", async () => {
    const m = menuFor("timeline.clip", { ids: [C1] });
    expect(m.item("Split at playhead").disabled).toBeUndefined();
    await m.choose("Split at playhead");
    expect(call).toHaveBeenCalledWith("clip.split", { item: C1, at: 5 * F });
    // Selecting a clip first: it is the selection now.
    expect(viewStore.getState().selection).toEqual([{ kind: "item", id: C1 }]);
  });

  it("disables Split with its reason when the playhead is outside the clip", () => {
    viewStore.setState({ playhead: 15 * F });
    const row = menuFor("timeline.clip", { ids: [C1] }).item("Split at playhead");
    expect(row.disabled).toBe(true);
    expect(row.disabledReason).toBe("The playhead is not inside the clip");
  });

  it("names a selection, and says why Rename and Split cannot run on several clips", () => {
    viewStore.getState().setSelection([C1, C2, C3].map((id) => ({ kind: "item", id }) as never));
    const m = menuFor("timeline.clip", { ids: [C2] });
    expect(m.menu.name).toBe("Clip actions for 3 clips");
    expect(m.item("Delete 3 clips").disabled).toBeUndefined();
    const rename = m.item("Rename…");
    expect(rename.disabled).toBe(true);
    expect(rename.disabledReason).toBe("Select one clip");
    expect(m.item("Split at playhead").disabledReason).toBe("Select one clip");
  });

  it("deletes the whole selection with one command", async () => {
    viewStore.getState().setSelection([C1, C2].map((id) => ({ kind: "item", id }) as never));
    await menuFor("timeline.clip", { ids: [C1] }).choose("Delete 2 clips");
    expect(call).toHaveBeenCalledWith("item.delete", { items: [C1, C2] });
  });

  it("disables Split, Delete and the rest of the editing rows on a locked track: Track is locked", () => {
    const m = menuFor("timeline.clip", { ids: [CL] });
    for (const label of [
      "Split at playhead",
      "Delete clip",
      /^Enable clip|^Disable clip/,
      "Rename…",
    ]) {
      const row = m.item(label);
      expect(row.disabled, String(label)).toBe(true);
      expect(row.disabledReason, String(label)).toBe("Track is locked");
    }
  });

  it("turns a clip off or on, one clip at a time, with item.set_props", async () => {
    await menuFor("timeline.clip", { ids: [C1] }).choose("Disable clip");
    expect(call).toHaveBeenCalledWith("item.set_props", { item: C1, props: { enabled: false } });
    call.mockClear();
    await menuFor("timeline.clip", { ids: [C3] }).choose("Enable clip");
    expect(call).toHaveBeenCalledWith("item.set_props", { item: C3, props: { enabled: true } });
  });

  it("opens the rename question, and reveals the clip's media in the bin", async () => {
    await menuFor("timeline.clip", { ids: [C1] }).choose("Rename…");
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "rename-clip", item: C1, name: "" });
    await menuFor("timeline.clip", { ids: [C1] }).choose("Reveal in media bin");
    expect(viewStore.getState().selection).toEqual([{ kind: "media", id: MED_V }]);
    expect(viewStore.getState().assetTab).toBe("media");
  });

  it("opens the same menu from a clip's edge, and keeps the selection on an empty lane", () => {
    expect(menuFor("timeline.edge", { ids: [C2] }).item("Delete clip")).toBeDefined();
    viewStore.getState().setSelection([{ kind: "item", id: C1 } as never]);
    menuFor("timeline.lane", { ids: [], place: { time: 3 * F, trackId: V1 } });
    expect(viewStore.getState().selection).toEqual([{ kind: "item", id: C1 }]);
  });
});

describe("the track, lane, ruler and playhead menus", () => {
  it("shows the track's flags as check rows and flips them with track.set", async () => {
    const m = menuFor("timeline.track", { ids: [A1] });
    expect(m.item("Unmute").checked).toBe(true);
    expect(m.item("Lock").checked).toBe(false);
    await m.choose("Unmute");
    expect(call).toHaveBeenCalledWith("track.set", { track: A1, muted: false });
    call.mockClear();
    await menuFor("timeline.track", { ids: [V1] }).choose("Lock");
    expect(call).toHaveBeenCalledWith("track.set", { track: V1, locked: true });
    expect(viewStore.getState().selection).toEqual([{ kind: "track", id: V1 }]);
  });

  it("adds a track above or below the one it is on, of the same kind", async () => {
    await menuFor("timeline.track", { ids: [V1] }).choose("Add track above");
    expect(call).toHaveBeenCalledWith("track.add", { sequence: SEQ, kind: "video", index: 1 });
    call.mockClear();
    await menuFor("timeline.lane", { place: { time: 0, trackId: A1 } }).choose("Add track below");
    expect(call).toHaveBeenCalledWith("track.add", { sequence: SEQ, kind: "audio", index: 3 });
  });

  it("deletes an empty track at once and asks first for one with clips", async () => {
    await menuFor("timeline.track", { ids: [V2] }).choose("Delete track");
    expect(call).toHaveBeenCalledWith("track.remove", { track: V2 });
    call.mockClear();
    await menuFor("timeline.track", { ids: [V1] }).choose("Delete track");
    expect(call).not.toHaveBeenCalled();
    expect(menuDialogStore.getState().dialog).toMatchObject({
      kind: "delete-track",
      track: V1,
      clips: 3,
    });
  });

  it("selects all clips on a track from the lane", async () => {
    await menuFor("timeline.lane", { place: { time: 0, trackId: V1 } }).choose(
      "Select all clips on track",
    );
    expect(viewStore.getState().selection).toEqual(
      [C1, C2, C3].map((id) => ({ kind: "item", id })),
    );
    expect(
      menuFor("timeline.lane", { place: { time: 0, trackId: V2 } }).item(
        "Select all clips on track",
      ).disabledReason,
    ).toBe("The track has no clips");
  });

  it("has Move playhead here on the ruler and the lane, Go to start and end, Copy timecode", () => {
    const ruler = menuFor("timeline.ruler", { place: { time: 3 * F } });
    for (const label of ["Move playhead here", "Go to start", "Go to end", "Copy timecode"]) {
      expect(ruler.item(label)).toBeDefined();
    }
    const playhead = menuFor("timeline.playhead", { place: { time: 5 * F } });
    expect(playhead.items.some((i) => i.label === "Move playhead here")).toBe(false);
    expect(playhead.item("Copy timecode")).toBeDefined();
  });
});

describe("the bin menus", () => {
  it("inserts at the playhead on the first free track that fits, as timeline.insert", async () => {
    viewStore.setState({ playhead: 4 * F + 1 });
    const m = menuFor("bin.card", { ids: [MED_V], name: "interview.mp4" });
    expect(m.menu.name).toBe("Media actions for interview.mp4");
    await m.choose("Insert at playhead");
    const [name, args] = call.mock.calls[0] as [string, Record<string, unknown>];
    expect(name).toBe("timeline.insert");
    expect(args).toMatchObject({ sequence: SEQ, track: V2, media: MED_V, overlap: "new_track" });
    expect(args.at).toBe(4 * F);
    expect(viewStore.getState().selection).toEqual([{ kind: "media", id: MED_V }]);
  });

  it("appends after the last clip of the sequence", async () => {
    await menuFor("bin.card", { ids: [MED_V] }).choose("Append to timeline");
    expect(call).toHaveBeenCalledWith("timeline.insert", {
      sequence: SEQ,
      track: V2,
      media: MED_V,
      at: 30 * F,
      overlap: "new_track",
    });
  });

  it("removes an unused asset at once and asks first for one clips use", async () => {
    const engineCall = vi.fn(async () => ({}));
    engineStore.setState({ rpc: { call: engineCall } as never, connection: "ready" });
    await menuFor("bin.card", { ids: [MED_U] }).choose("Remove from project");
    expect(engineCall).toHaveBeenCalledWith("media.remove", { id: MED_U });
    engineCall.mockClear();
    await menuFor("bin.card", { ids: [MED_V] }).choose("Remove from project");
    expect(engineCall).not.toHaveBeenCalled();
    expect(menuDialogStore.getState().dialog).toMatchObject({
      kind: "remove-media",
      id: MED_V,
      uses: 4,
    });
    engineStore.setState({ rpc: null, connection: "none" });
  });

  it("plays in the Source view only what plays, and reveals files by their path", () => {
    const m = menuFor("bin.card", { ids: [MED_V] });
    expect(m.item("Play in Source view").disabled).toBeUndefined();
    expect(m.item(/^Reveal in /).disabled).toBeUndefined();
    expect(m.item("Copy path").disabled).toBeUndefined();
    const sequenceCard = menuFor("bin.card", { ids: [hex("med", 99)] });
    expect(sequenceCard.item("Play in Source view").disabled).toBe(true);
    expect(sequenceCard.item(/^Reveal in /).disabledReason).toBe("No file on disk");
  });

  it("clears the selection on an empty bin and offers Import, Sort by, View as, hover scrub", () => {
    viewStore.getState().setSelection([{ kind: "media", id: MED_V } as never]);
    const m = menuFor("bin.background");
    expect(viewStore.getState().selection).toEqual([]);
    expect(m.item("Import…").disabled).toBeUndefined();
    expect(m.item("Newest first").checked).toBe(true);
    expect(m.item("Name, A to Z").checked).toBe(false);
    expect(m.item("Grid").checked).toBe(true);
    expect(m.item("Show hover scrub")).toBeDefined();
  });

  it("sets the sort and the layout through the bin's own state", async () => {
    await menuFor("bin.background").choose("Name, A to Z");
    expect(menuFor("bin.background").item("Name, A to Z").checked).toBe(true);
    await menuFor("bin.background").choose("List");
    expect(layoutStore.getState().layout).toBe("list");
    layoutStore.getState().set("grid");
  });
});

describe("the other surfaces", () => {
  it("collapses and expands every Inspector section", async () => {
    await menuFor("inspector.section", { name: "Clip" }).choose("Collapse all");
    expect(sectionsStore.getState().closed.size).toBeGreaterThan(3);
    await menuFor("inspector.section", { name: "Clip" }).choose("Expand all");
    expect(sectionsStore.getState().closed.size).toBe(0);
  });

  it("has the dock tab's rows, and the empty dock's", () => {
    const tab = menuFor("dock.tab", { ids: ["inspector"], name: "Inspector" });
    expect(tab.menu.name).toBe("Panel actions for Inspector");
    for (const label of [
      "Close panel",
      "Maximize panel",
      "Float panel",
      "Left",
      "Right",
      "Bottom",
    ]) {
      expect(tab.item(label)).toBeDefined();
    }
    expect(menuFor("dock.empty").item("Reopen all panels")).toBeDefined();
  });

  it("sends Cancel export as job.cancel with the running job, and only then", async () => {
    const m = menuFor("chrome.export");
    expect(m.item("Cancel export").disabledReason).toBe("No export is running");
  });

  it("names Undo and Redo and disables them when there is nothing to undo", () => {
    const m = menuFor("chrome.history");
    expect(m.item("Undo").disabledReason).toBe("Nothing to undo");
    expect(m.item("Redo").disabledReason).toBe("Nothing to redo");
  });

  it("copies a setting's key and says Reset to default needs a changed value", () => {
    const m = menuFor("settings.row", {
      name: "Autosave",
      data: { key: "saving.autosave", modified: false },
    });
    expect(m.item("Reset to default").disabledReason).toBe("Already the default");
    expect(m.item("Copy setting key")).toBeDefined();
  });
});

describe("every row's command and arguments", () => {
  it("names a catalog command with arguments that fit its schema, on a realistic target", async () => {
    const targets: Record<string, Partial<MenuTarget>> = {
      "timeline.clip": { ids: [C1] },
      "timeline.edge": { ids: [C1] },
      "timeline.lane": { place: { time: 5 * F, trackId: V1 } },
      "timeline.track": { ids: [V1] },
      "bin.card": { ids: [MED_V] },
      "chrome.history": {},
    };
    let checked = 0;
    for (const [surface, target] of Object.entries(targets)) {
      const def = services.surfaces.get(surface);
      const ctxMenu = menuFor(surface, target);
      for (const row of def?.rows ?? []) {
        const command = row.command;
        if (command === undefined || services.ui.get(command) !== undefined) continue;
        const item = ctxMenu.items.find((i) => i.id === (row.id ?? command));
        if (item === undefined || item.disabled === true) continue;
        call.mockClear();
        await ctxMenu.choose(item.label);
        const [name, args] = call.mock.calls[0] ?? [];
        expect(name, `${surface}/${row.id ?? command}`).toBe(command);
        expect(schemaProblems(command, args), `${surface}/${row.id ?? command}`).toEqual([]);
        checked += 1;
      }
    }
    expect(checked).toBeGreaterThan(8);
  });
});

describe("a choice is never lost", () => {
  it("runs the chosen command even when a second right-click opens another menu before the first closed", async () => {
    render(
      h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(ContextMenuHost, { services }))),
    );
    act(() => {
      openContextMenu(
        { surface: "timeline.track", point: { x: 10, y: 10 }, target: { ids: [A1] } },
        services,
      );
    });
    const item = await screen.findByRole("menuitemcheckbox", { name: /^Unmute/ });
    act(() => {
      item.click();
      openContextMenu(
        { surface: "timeline.track", point: { x: 10, y: 10 }, target: { ids: [V1] } },
        services,
      );
    });
    await vi.waitFor(() =>
      expect(call).toHaveBeenCalledWith("track.set", { track: A1, muted: false }),
    );
  });
});

describe("accessibility", () => {
  const TARGETS: Record<string, Partial<MenuTarget>> = {
    "dock.tab": { ids: ["inspector"], name: "Inspector" },
    "bin.card": { ids: [MED_V], name: "interview.mp4" },
    "bin.chip": { ids: ["kind-video"], name: "Video" },
    "timeline.clip": { ids: [C1] },
    "timeline.edge": { ids: [C1] },
    "timeline.lane": { place: { time: 0, trackId: V1 } },
    "timeline.track": { ids: [V1] },
    "timeline.ruler": { place: { time: 0 } },
    "timeline.playhead": { place: { time: 0 } },
    "inspector.section": { name: "Clip" },
    "inspector.row": { name: "Name", data: { text: "x" } },
    "inspector.path": { ids: [MED_V] },
    "inspector.picture": { ids: [MED_V] },
    "settings.row": { name: "Autosave", data: { key: "saving.autosave", modified: true } },
    "chrome.toast": { ids: ["1"], data: { text: "x" } },
  };

  it("has no axe violations on any surface's menu", async () => {
    // The host with this test's services; every surface of the app, opened as a person would.
    render(
      h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(ContextMenuHost, { services }))),
    );
    const found: string[] = [];
    for (const def of SURFACES) {
      let opened = false;
      act(() => {
        opened = openContextMenu(
          { surface: def.id, point: { x: 40, y: 40 }, target: TARGETS[def.id] ?? {} },
          services,
        );
      });
      expect(opened, def.id).toBe(true);
      await screen.findByRole("menu");
      const results = await axe.run(document.body, {
        rules: { "color-contrast": { enabled: false }, region: { enabled: false } },
      });
      for (const v of results.violations) found.push(`${def.id}: ${v.id}: ${v.nodes[0]?.html}`);
      act(() => closeContextMenu());
    }
    expect(found).toEqual([]);
  }, 120_000);
});

describe("the folder menus (WP-2F.08)", () => {
  const FLD_A = hex("fld", 1);
  const FLD_B = hex("fld", 2);

  const withFolders = () => {
    const p = project();
    (p as unknown as Record<string, unknown>).folders = {
      [FLD_A]: { id: FLD_A, name: "Interviews", parent: null, custom: {} },
      [FLD_B]: { id: FLD_B, name: "B-roll", parent: null, custom: {} },
    };
    docStore.setState({ status: "ready", rev: 2, committedRev: 2, project: p });
  };

  it("a card offers Rename and Move to… and names a selection when removing several", () => {
    const one = menuFor("bin.card", { ids: [MED_V] });
    expect(one.item("Rename").disabled).toBeUndefined();
    expect(one.item("Move to…").disabled).toBeUndefined();
    expect(one.items.some((i) => /^Remove \d+ from project$/.test(i.label))).toBe(false);
    const many = menuFor("bin.card", { ids: [MED_V, MED_U] });
    expect(many.item("Rename").disabled).toBe(true);
    expect(many.item("Rename").disabledReason).toBe("Select one item");
    expect(many.item("Remove 2 from project")).toBeDefined();
    expect(many.items.some((i) => i.label === "Remove from project")).toBe(false);
  });

  it("Rename starts the name edit in place and Move to… opens the folder picker", async () => {
    withFolders();
    await menuFor("bin.card", { ids: [MED_V] }).choose("Rename");
    expect(folderUiStore.getState().renaming).toEqual({ kind: "media", id: MED_V });
    folderUiStore.getState().stopRename();
    await menuFor("bin.card", { ids: [MED_V, MED_U] }).choose("Move to…");
    expect(menuDialogStore.getState().dialog).toEqual({
      kind: "move-to",
      media: [MED_V, MED_U],
      folders: [],
    });
  });

  it("a folder tile has its own menu: Open, New folder, Rename, Move to…, Delete folder…", async () => {
    withFolders();
    const m = menuFor("bin.folder", { ids: [FLD_A], name: "Interviews" });
    expect(m.menu.name).toBe("Folder actions for Interviews");
    for (const label of ["Open", "New folder", "Rename", "Move to…", "Delete folder…"]) {
      expect(m.item(label).disabled, label).toBeUndefined();
    }
    await m.choose("Rename");
    expect(folderUiStore.getState().renaming).toEqual({ kind: "folder", id: FLD_A });
    folderUiStore.getState().stopRename();
  });

  it("several folders: no Open or Rename, and Delete names the count", () => {
    withFolders();
    folderUiStore.getState().setFolders([FLD_A as never, FLD_B as never]);
    const m = menuFor("bin.folder", { ids: [FLD_A] });
    expect(m.menu.name).toBe("Folder actions for 2 folders");
    expect(m.item("Open").disabled).toBe(true);
    expect(m.item("Rename").disabled).toBe(true);
    expect(m.item("Delete 2 folders…").disabled).toBeUndefined();
    folderUiStore.getState().setFolders([]);
  });

  it("Open goes into the folder", async () => {
    withFolders();
    await menuFor("bin.folder", { ids: [FLD_B] }).choose("Open");
    expect(currentOpenFolder()).toBe(FLD_B);
    binStore.getState().openFolder("", null);
  });

  it("the empty bin offers New folder and Select all", () => {
    withFolders();
    const m = menuFor("bin.background");
    expect(m.item("New folder").disabled).toBeUndefined();
    expect(m.item("Select all").disabled).toBeUndefined();
  });

  it("Select all selects the assets and folders the bin shows", async () => {
    withFolders();
    await menuFor("bin.background").choose("Select all");
    expect(folderUiStore.getState().folders).toEqual([FLD_B, FLD_A]);
    expect(viewStore.getState().selection.map((r) => r.id)).toEqual([MED_V, MED_A, MED_U]);
    folderUiStore.getState().setFolders([]);
  });
});
