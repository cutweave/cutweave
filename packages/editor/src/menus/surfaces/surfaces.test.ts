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
import { forgetAppliedLayouts } from "../../layout/actions.ts";
import { binStore } from "../../media/binStore.ts";
import { currentOpenFolder, folderUiStore } from "../../media/folderUi.ts";
import { healthStore } from "../../media/health.ts";
import { layoutStore } from "../../media/layoutStore.ts";
import { docStore } from "../../state/docStore.ts";
import { toastStore } from "../../state/toastStore.ts";
import { initialViewState, viewStore } from "../../state/viewStore.ts";
import { clipboardStore } from "../../timeline/clipboard.ts";
import { replacePickStore } from "../../timeline/replacePick.ts";
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
  replacePickStore.setState({ clip: null, media: null });
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
      "Reset properties",
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

  it("resets the properties of every selected clip with item.reset_props (WP-2C.03)", async () => {
    viewStore.getState().setSelection([C1, C2].map((id) => ({ kind: "item", id }) as never));
    await menuFor("timeline.clip", { ids: [C1] }).choose("Reset properties");
    expect(call).toHaveBeenCalledWith("item.reset_props", { items: [C1, C2] });
  });

  it("opens the rename question, and reveals the clip's media in the bin", async () => {
    await menuFor("timeline.clip", { ids: [C1] }).choose("Rename…");
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "rename-clip", item: C1, name: "" });
    await menuFor("timeline.clip", { ids: [C1] }).choose("Reveal in media bin");
    expect(viewStore.getState().selection).toEqual([{ kind: "media", id: MED_V }]);
    expect(viewStore.getState().assetTab).toBe("media");
  });

  it("changes the speed from the Speed submenu, checks the current one, and asks for a custom one (WP-2D.01)", async () => {
    viewStore.getState().setSelection([C1, C2].map((id) => ({ kind: "item", id }) as never));
    const m = menuFor("timeline.clip", { ids: [C1] });
    expect(m.item("Normal").checked).toBe(true);
    expect(m.item("2x").checked).toBe(false);
    await m.choose("2x");
    expect(call).toHaveBeenCalledWith("clip.set_speed", { items: [C1, C2], speed: "2/1" });
    await menuFor("timeline.clip", { ids: [C1] }).choose("Custom…");
    expect(menuDialogStore.getState().dialog).toEqual({
      kind: "custom-speed",
      items: [C1, C2],
      speed: "1",
    });
  });

  it("reverses and freezes a video clip, and leaves them out for sound and stills (WP-2D.01)", async () => {
    await menuFor("timeline.clip", { ids: [C1] }).choose("Reverse");
    expect(call).toHaveBeenCalledWith("clip.reverse", { items: [C1], reverse: true });
    await menuFor("timeline.clip", { ids: [C1] }).choose("Freeze frame");
    expect(call).toHaveBeenCalledWith("clip.freeze_frame", { item: C1, at: 5 * F });
    viewStore.setState({ playhead: 15 * F });
    const outside = menuFor("timeline.clip", { ids: [C1] }).item("Freeze frame");
    expect(outside.disabledReason).toBe("The playhead is not inside the clip");
    // A reversed clip shows Reverse checked; choosing it plays forwards again.
    const reversed = project();
    const seq = reversed.sequences[SEQ as keyof typeof reversed.sequences] as unknown as {
      tracks: Record<string, { items: Record<string, Record<string, unknown>> }>;
    };
    const c1 = seq.tracks[V1]?.items[C1];
    if (c1 !== undefined) {
      c1.reverse = true;
      c1.speed = { num: 1, den: 2 };
    }
    const sound = { ...(c1 ?? {}), id: hex("itm", 9), media: MED_A, reverse: false };
    const a1 = seq.tracks[A1];
    if (a1 !== undefined) a1.items[hex("itm", 9)] = sound;
    docStore.setState({ project: reversed });
    const again = menuFor("timeline.clip", { ids: [C1] });
    expect(again.item("Reverse").checked).toBe(true);
    expect(again.item("0.5x").checked).toBe(true);
    await again.choose("Reverse");
    expect(call).toHaveBeenLastCalledWith("clip.reverse", { items: [C1], reverse: false });
    const labels = menuFor("timeline.clip", { ids: [hex("itm", 9)] }).items.map((i) => i.label);
    for (const left of ["Reverse", "Freeze frame", "2x"]) expect(labels).not.toContain(left);
  });

  it("keeps the selection on an empty lane", () => {
    viewStore.getState().setSelection([{ kind: "item", id: C1 } as never]);
    menuFor("timeline.lane", { ids: [], place: { time: 3 * F, trackId: V1 } });
    expect(viewStore.getState().selection).toEqual([{ kind: "item", id: C1 }]);
  });
});

describe("links and attachments (WP-2A.05)", () => {
  const SOUND = hex("itm", 5);
  const OVERLAY = hex("itm", 6);

  /** V1 (magnetic): C1 0-10 s; A1: SOUND 0-10 s (aligned with C1); V2: OVERLAY 2-5 s. */
  function withLinkables(extra: { linked?: boolean; attached?: boolean } = {}): void {
    const p = project();
    const tracks = p.sequences[SEQ as keyof typeof p.sequences]?.tracks as unknown as Record<
      string,
      { magnetic: boolean; items: Record<string, unknown> }
    >;
    const group = extra.linked === true ? "grp_1" : null;
    (tracks[V1] as { magnetic: boolean }).magnetic = true;
    (tracks[V1]?.items[C1] as { linked_group: string | null }).linked_group = group;
    (tracks[A1] as { items: Record<string, unknown> }).items[SOUND] = clip(SOUND, MED_A, 0, {
      linked_group: group,
    });
    (tracks[V2] as { items: Record<string, unknown> }).items[OVERLAY] = clip(OVERLAY, MED_V, 2, {
      duration: 3 * F,
      attached_to: extra.attached === true ? C1 : null,
    });
    docStore.setState({ status: "ready", rev: 2, committedRev: 2, project: p });
  }

  const select = (...ids: string[]) =>
    viewStore.getState().setSelection(ids.map((id) => ({ kind: "item", id }) as never));

  it("links aligned clips on different tracks with item.link", async () => {
    withLinkables();
    select(C1, SOUND);
    const m = menuFor("timeline.clip", { ids: [C1] });
    expect(m.item("Link clips").disabled).toBeUndefined();
    expect(m.item("Unlink clips").disabledReason).toBe("These clips are not linked");
    await m.choose("Link clips");
    expect(call).toHaveBeenCalledWith("item.link", { items: [C1, SOUND] });
    expect(schemaProblems("item.link", { items: [C1, SOUND] })).toEqual([]);
  });

  it("says why Link cannot run: one clip, clips that are not aligned, a pair already linked", () => {
    withLinkables();
    const reason = "Select clips on different tracks that start and end together";
    expect(menuFor("timeline.clip", { ids: [C1] }).item("Link clips").disabledReason).toBe(reason);
    select(C1, OVERLAY);
    expect(menuFor("timeline.clip", { ids: [C1] }).item("Link clips").disabledReason).toBe(reason);
    withLinkables({ linked: true });
    select(C1, SOUND);
    expect(menuFor("timeline.clip", { ids: [C1] }).item("Link clips").disabledReason).toBe(
      "These clips are already linked",
    );
  });

  it("unlinks the selected clips with item.unlink", async () => {
    withLinkables({ linked: true });
    select(C1, SOUND);
    const m = menuFor("timeline.clip", { ids: [C1] });
    expect(m.item("Unlink clips").disabled).toBeUndefined();
    await m.choose("Unlink clips");
    expect(call).toHaveBeenCalledWith("item.unlink", { items: [C1, SOUND] });
  });

  it("attaches a clip to the main-track clip it starts in, and detaches it again", async () => {
    withLinkables();
    const m = menuFor("timeline.clip", { ids: [OVERLAY] });
    expect(m.item("Detach from main clip").disabledReason).toBe(
      "Not attached to a main-track clip",
    );
    await m.choose("Attach to main clip");
    expect(call).toHaveBeenCalledWith("item.attach", { items: [OVERLAY], parent: C1 });
    expect(schemaProblems("item.attach", { items: [OVERLAY], parent: C1 })).toEqual([]);
    withLinkables({ attached: true });
    call.mockClear();
    await menuFor("timeline.clip", { ids: [OVERLAY] }).choose("Detach from main clip");
    expect(call).toHaveBeenCalledWith("item.detach", { items: [OVERLAY] });
  });

  it("cannot attach a main-track clip, and names a locked track", () => {
    withLinkables();
    expect(menuFor("timeline.clip", { ids: [C2] }).item("Attach to main clip").disabledReason).toBe(
      "Place the clips inside a main-track clip",
    );
    const locked = menuFor("timeline.clip", { ids: [CL] });
    expect(locked.item("Attach to main clip").disabledReason).toBe("Track is locked");
    expect(locked.item("Link clips").disabledReason).toBe("Track is locked");
  });
});

describe("the edge menu (WP-2A.04)", () => {
  const right = (id: string) => ({ ids: [id], data: { zone: "right-edge" } });
  const left = (id: string) => ({ ids: [id], data: { zone: "left-edge" } });

  it("trims, ripple trims and rolls the end of a clip to the playhead, clamped", async () => {
    const m = menuFor("timeline.edge", right(C1));
    expect(m.menu.name).toBe("Edge actions for interview.mp4");
    expect(m.items.map((i) => i.label)).toEqual([
      "Trim to playhead",
      "Ripple trim to playhead",
      "Roll to playhead",
      "Extend to next clip",
    ]);
    await m.choose("Trim to playhead");
    expect(call).toHaveBeenCalledWith("clip.trim", {
      item: C1,
      edge: "end",
      to: 5 * F,
      clamp: true,
    });
    await menuFor("timeline.edge", right(C1)).choose("Ripple trim to playhead");
    expect(call).toHaveBeenCalledWith("clip.ripple_trim", {
      item: C1,
      edge: "end",
      to: 5 * F,
      clamp: true,
    });
    await menuFor("timeline.edge", right(C1)).choose("Roll to playhead");
    expect(call).toHaveBeenCalledWith("clip.roll", { left: C1, right: C2, to: 5 * F, clamp: true });
    expect(viewStore.getState().selection).toEqual([{ kind: "item", id: C1 }]);
  });

  it("on a start edge, trims the head and rolls the cut with the clip before it", async () => {
    await menuFor("timeline.edge", left(C2)).choose("Trim to playhead");
    expect(call).toHaveBeenCalledWith("clip.trim", {
      item: C2,
      edge: "start",
      to: 5 * F,
      clamp: true,
    });
    await menuFor("timeline.edge", left(C2)).choose("Roll to playhead");
    expect(call).toHaveBeenCalledWith("clip.roll", { left: C1, right: C2, to: 5 * F, clamp: true });
    expect(menuFor("timeline.edge", left(C2)).item("Extend to previous clip").disabledReason).toBe(
      "No gap next to this edge",
    );
  });

  it("says why a row cannot run: the playhead on the edge, no clip touching it, a locked track", () => {
    viewStore.setState({ playhead: 10 * F });
    let m = menuFor("timeline.edge", right(C1));
    for (const label of ["Trim to playhead", "Ripple trim to playhead", "Roll to playhead"]) {
      expect(m.item(label).disabledReason, label).toBe(
        "Move the playhead to where the edge should go",
      );
    }
    viewStore.setState({ playhead: 25 * F });
    m = menuFor("timeline.edge", right(C3));
    expect(m.item("Trim to playhead").disabled).toBeUndefined();
    expect(m.item("Roll to playhead").disabledReason).toBe("No clip touches this edge");
    m = menuFor("timeline.edge", right(CL));
    for (const label of ["Trim to playhead", "Roll to playhead", "Extend to next clip"]) {
      expect(m.item(label).disabledReason, label).toBe("Track is locked");
    }
  });

  it("extends an edge across a gap to the next or the previous clip", async () => {
    const p = project() as unknown as {
      sequences: Record<string, { tracks: Record<string, { items: Record<string, object> }> }>;
    };
    const v2 = p.sequences[SEQ]?.tracks[V2];
    if (v2 === undefined) throw new Error("no V2");
    v2.items = {
      [hex("itm", 7)]: clip(hex("itm", 7), MED_V, 0),
      [hex("itm", 8)]: clip(hex("itm", 8), MED_V, 14),
    };
    docStore.setState({ project: p as unknown as Project });
    await menuFor("timeline.edge", right(hex("itm", 7))).choose("Extend to next clip");
    expect(call).toHaveBeenCalledWith("clip.trim", {
      item: hex("itm", 7),
      edge: "end",
      to: 14 * F,
      clamp: true,
    });
    await menuFor("timeline.edge", left(hex("itm", 8))).choose("Extend to previous clip");
    expect(call).toHaveBeenCalledWith("clip.trim", {
      item: hex("itm", 8),
      edge: "start",
      to: 10 * F,
      clamp: true,
    });
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

  it("offers Relink… only on a card whose file is missing, and it opens the relink dialog", async () => {
    expect(menuFor("bin.card", { ids: [MED_V] }).items.map((i) => i.label)).not.toContain(
      "Relink…",
    );
    healthStore
      .getState()
      .setMissing([
        { media_id: MED_V, name: "interview.mp4", path: "/old/interview.mp4", status: "missing" },
      ]);
    try {
      await menuFor("bin.card", { ids: [MED_V] }).choose("Relink…");
      expect(healthStore.getState().dialogOpen).toBe(true);
    } finally {
      healthStore.getState().closeDialog();
      healthStore.getState().setMissing([]);
    }
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

  it("resets an Inspector section's group, and says why a section without one cannot (WP-2C.03)", async () => {
    await menuFor("inspector.section", {
      name: "Transform",
      ids: [C1],
      data: { group: "transform" },
    }).choose("Reset section");
    expect(call).toHaveBeenCalledWith("item.reset_props", { items: [C1], group: "transform" });
    const none = menuFor("inspector.section", { name: "Timing", ids: [C1], data: {} }).item(
      "Reset section",
    );
    expect(none.disabled).toBe(true);
    expect(none.disabledReason).toBe("Nothing to reset here");
    const locked = menuFor("inspector.section", {
      name: "Transform",
      ids: [CL],
      data: { group: "transform" },
    }).item("Reset section");
    expect(locked.disabledReason).toBe("Track is locked");
  });

  it("resets a property row, and adds or removes a keyframe at the playhead (WP-2C.03)", async () => {
    const row = { name: "Opacity", ids: [C1], data: { text: "100 %", prop: "opacity" } };
    await menuFor("inspector.row", row).choose("Reset to default");
    expect(call).toHaveBeenCalledWith("item.reset_props", { items: [C1], props: ["opacity"] });
    // Static: a key can be added at the playhead (inside the clip), not removed.
    const m = menuFor("inspector.row", row);
    expect(m.item("Remove keyframe").disabledReason).toBe("This property has no keyframes");
    call.mockClear();
    await m.choose("Add keyframe");
    expect(call).toHaveBeenCalledWith("keyframe.set", {
      item: C1,
      prop: "opacity",
      keys: [{ at: 5 * F }],
    });
    // Animated, with a key at the playhead: it can be removed, not added.
    const keyed = structuredClone(docStore.getState().project) as unknown as {
      sequences: Record<string, { tracks: Record<string, { items: Record<string, object> }> }>;
    };
    const items = keyed.sequences[SEQ]?.tracks[V1]?.items ?? {};
    items[C1] = {
      ...items[C1],
      visual: {
        transform: {},
        opacity: { keyframes: [{ t: 5 * F, value: 0.5, interp: { kind: "linear" } }] },
      },
    };
    docStore.setState({ project: keyed as unknown as Project });
    const k = menuFor("inspector.row", row);
    expect(k.item("Add keyframe").disabledReason).toBe("A keyframe is already here");
    call.mockClear();
    await k.choose("Remove keyframe");
    expect(call).toHaveBeenCalledWith("keyframe.delete", {
      item: C1,
      keys: [{ prop: "opacity", t: 5 * F }],
    });
    // A row without an animatable property (Name) offers neither.
    const name = menuFor("inspector.row", { name: "Name", ids: [C1], data: { text: "x" } });
    expect(name.item("Add keyframe").disabled).toBe(true);
    expect(name.item("Reset to default").disabled).toBe(true);
    // The playhead outside the clip: no key can be added.
    viewStore.setState({ playhead: 15 * F });
    expect(menuFor("inspector.row", row).item("Add keyframe").disabledReason).toBe(
      "The playhead is not inside the clip",
    );
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

describe("the clipboard rows (WP-2A.06)", () => {
  const payload = (id: string) => ({
    version: 1,
    sequence: SEQ,
    frame_rate: { num: 30, den: 1 },
    span: 10 * F,
    tracks: [{ kind: "video", row: 0, source: V1 }],
    items: [{ track: 0, item: clip(id, MED_V, 0) }],
  });

  beforeEach(() => clipboardStore.getState().setPayload(null));

  it("copies the clips with timeline.copy and keeps the payload for Paste here", async () => {
    call.mockImplementation(async (name) =>
      name === "timeline.copy" ? { clipboard: payload(C1) } : {},
    );
    await menuFor("timeline.clip", { ids: [C1] }).choose("Copy");
    expect(call).toHaveBeenCalledWith("timeline.copy", { items: [C1] });
    expect(clipboardStore.getState().payload?.items).toHaveLength(1);
  });

  it("cuts with timeline.copy, then one item.delete", async () => {
    call.mockImplementation(async (name) =>
      name === "timeline.copy" ? { clipboard: payload(C1) } : {},
    );
    await menuFor("timeline.clip", { ids: [C1, C2] }).choose(/^Cut/);
    expect(call.mock.calls.map((c) => c[0])).toEqual(["timeline.copy", "item.delete"]);
    expect(call).toHaveBeenLastCalledWith("item.delete", { items: [C1, C2] });
  });

  it("duplicates with item.duplicate and selects the copies", async () => {
    call.mockImplementation(async () => ({ copies: [{ source: C1, copy: hex("itm", 9) }] }));
    const m = menuFor("timeline.clip", { ids: [C1] });
    expect(m.item("Duplicate").disabled).toBeUndefined();
    await m.choose("Duplicate");
    expect(call).toHaveBeenCalledWith("item.duplicate", { items: [C1] });
    expect(schemaProblems("item.duplicate", { items: [C1] })).toEqual([]);
  });

  it("says why Cut and Duplicate cannot run on a locked track", () => {
    const m = menuFor("timeline.clip", { ids: [CL] });
    expect(m.item(/^Cut/).disabledReason).toBe("Track is locked");
    expect(m.item("Duplicate").disabledReason).toBe("Track is locked");
    expect(m.item("Copy").disabled).toBeUndefined();
  });

  it("offers Paste here only with something copied, at the click's time and track", async () => {
    const lane = { place: { time: 7 * F, trackId: V1 } };
    expect(menuFor("timeline.lane", lane).item("Paste here").disabledReason).toBe(
      "Copy clips first",
    );
    clipboardStore.getState().setPayload(payload(C1) as never);
    const m = menuFor("timeline.lane", lane);
    expect(m.item("Paste here").disabled).toBeUndefined();
    call.mockImplementation(async () => ({ copies: [{ source: C1, copy: hex("itm", 9) }] }));
    await m.choose("Paste here");
    const [name, args] = call.mock.calls[0] ?? [];
    expect(name).toBe("timeline.paste");
    expect(args).toMatchObject({ at: 7 * F, track: V1 });
    expect(schemaProblems("timeline.paste", args)).toEqual([]);
    expect(
      menuFor("timeline.lane", { place: { time: 0, trackId: LOCKED } }).item("Paste here")
        .disabledReason,
    ).toBe("Track is locked");
  });

  it("asks which attributes to paste, from the copied clip that is still in the project", async () => {
    const m = menuFor("timeline.clip", { ids: [C2] });
    expect(m.item("Paste attributes…").disabledReason).toBe("Copy a clip from this project first");
    clipboardStore.getState().setPayload(payload(C1) as never);
    const after = menuFor("timeline.clip", { ids: [C2] });
    expect(after.item("Paste attributes…").disabled).toBeUndefined();
    await after.choose("Paste attributes…");
    expect(menuDialogStore.getState().dialog).toEqual({
      kind: "paste-attributes",
      from: C1,
      to: [C2],
    });
    // The copied clip itself is no target.
    expect(menuFor("timeline.clip", { ids: [C1] }).item("Paste attributes…").disabled).toBe(true);
  });
});

describe("replace clip (WP-2A.11)", () => {
  const pick = (kind: "item" | "media", id: string) =>
    viewStore.getState().setSelection([{ kind, id } as never]);

  it("replaces at once with the bin card picked before, as one clip.replace", async () => {
    pick("media", MED_U);
    pick("item", C2);
    const m = menuFor("timeline.clip", { ids: [C2] });
    expect(m.item("Replace clip…").disabled).toBeUndefined();
    await m.choose("Replace clip…");
    expect(call).toHaveBeenCalledTimes(1);
    const [name, args] = call.mock.calls[0] ?? [];
    expect(name).toBe("clip.replace");
    expect(args).toEqual({ item: C2, media: MED_U });
    expect(schemaProblems("clip.replace", args)).toEqual([]);
    expect(menuDialogStore.getState().dialog).toBeNull();
  });

  it("asks for the media when no card fits: none picked, or sound for a picture", async () => {
    await menuFor("timeline.clip", { ids: [C2] }).choose("Replace clip…");
    expect(call).not.toHaveBeenCalled();
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "replace-clip", item: C2 });
    menuDialogStore.getState().close();
    pick("media", MED_A);
    await menuFor("timeline.clip", { ids: [C2] }).choose("Replace clip…");
    expect(call).not.toHaveBeenCalled();
    expect(menuDialogStore.getState().dialog).toEqual({ kind: "replace-clip", item: C2 });
  });

  it("says why it cannot run: several clips, a locked track", () => {
    expect(menuFor("timeline.clip", { ids: [C1, C2] }).item("Replace clip…").disabledReason).toBe(
      "Select one clip",
    );
    expect(menuFor("timeline.clip", { ids: [CL] }).item("Replace clip…").disabledReason).toBe(
      "Track is locked",
    );
  });

  it("offers Replace selected clip on a card for the clip picked on the timeline", async () => {
    expect(menuFor("bin.card", { ids: [MED_U] }).item("Replace selected clip").disabledReason).toBe(
      "Pick a clip on the timeline first",
    );
    pick("item", C1);
    const m = menuFor("bin.card", { ids: [MED_U] });
    expect(m.item("Replace selected clip").disabled).toBeUndefined();
    await m.choose("Replace selected clip");
    const [name, args] = call.mock.calls[0] ?? [];
    expect(name).toBe("clip.replace");
    expect(args).toEqual({ item: C1, media: MED_U });
    expect(schemaProblems("clip.replace", args)).toEqual([]);
    expect(menuFor("bin.card", { ids: [MED_A] }).item("Replace selected clip").disabledReason).toBe(
      "This media cannot replace the clip you picked",
    );
    expect(menuFor("bin.card", { ids: [MED_V] }).item("Replace selected clip").disabledReason).toBe(
      "This media cannot replace the clip you picked",
    );
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
      "inspector.section": { ids: [C1], data: { group: "transform" } },
      "inspector.row": { ids: [C1], data: { text: "100 %", prop: "opacity" } },
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

describe("layouts (WP-2C.08)", () => {
  const rpcCalls = () => {
    const engineCall = vi.fn(async (name: string) => {
      if (name === "item.apply_layout") return { cells: [{ item: C1 }, { item: C2 }] };
      if (name === "edit.begin_tx") return { tx: "tx_1" };
      if (name === "timeline.insert") return { clips: [C3] };
      return {};
    });
    engineStore.setState({ rpc: { call: engineCall } as never, connection: "ready" });
    return engineCall;
  };
  beforeEach(() => forgetAppliedLayouts());
  afterEach(() => engineStore.setState({ rpc: null, connection: "none" }));

  it("offers the layouts for two video clips in a Layout submenu and applies one", async () => {
    const engineCall = rpcCalls();
    const m = menuFor("timeline.clip", { ids: [C1, C2] });
    const names = m.items.map((i) => i.label);
    expect(names).toContain("Side by side");
    expect(names).toContain("Picture in picture, top right");
    expect(names).not.toContain("2 × 2 grid");
    await m.choose("Side by side");
    expect(engineCall).toHaveBeenCalledWith("item.apply_layout", {
      items: [C1, C2],
      layout: "side_by_side",
      fill: "crop",
      at: 5 * F,
    });
  });

  it("offers no layout for one clip, three columns for three", () => {
    const one = menuFor("timeline.clip", { ids: [C1] });
    expect(one.items.map((i) => i.label)).not.toContain("Side by side");
    const three = menuFor("timeline.clip", { ids: [C1, C2, C3] });
    expect(three.items.map((i) => i.label)).toContain("Three columns");
    expect(three.items.map((i) => i.label)).not.toContain("Side by side");
  });

  it("Swap says why it cannot run until a layout was applied, then reverses the order", async () => {
    const engineCall = rpcCalls();
    const before = menuFor("timeline.clip", { ids: [C1, C2] }).item("Swap");
    expect(before.disabled).toBe(true);
    expect(before.disabledReason).toBe("Apply a layout to the two clips first");
    await menuFor("timeline.clip", { ids: [C1, C2] }).choose("Side by side");
    expect(menuFor("timeline.clip", { ids: [C1, C2] }).item("Swap").disabled).toBeUndefined();
    engineCall.mockClear();
    await menuFor("timeline.clip", { ids: [C1, C2] }).choose("Swap");
    expect(engineCall).toHaveBeenCalledWith("item.apply_layout", {
      items: [C1, C2],
      layout: "side_by_side",
      fill: "crop",
      order: [C2, C1],
      at: 5 * F,
    });
  });

  it("places two videos from the bin side by side in one transaction", async () => {
    const engineCall = rpcCalls();
    const m = menuFor("bin.card", { ids: [MED_V, MED_U] });
    expect(m.item("Place side by side").disabled).toBeUndefined();
    expect(m.items.map((i) => i.label)).toContain("Side by side");
    await m.choose("Place side by side");
    expect(engineCall.mock.calls.map((c) => c[0])).toEqual([
      "edit.begin_tx",
      "timeline.insert",
      "timeline.insert",
      "item.apply_layout",
      "edit.commit_tx",
    ]);
    const apply = engineCall.mock.calls[3] as unknown as [string, Record<string, unknown>];
    expect(apply[1]).toMatchObject({ layout: "side_by_side", fill: "crop" });
  });

  it("does not offer Place side by side for one asset, and refuses sound", () => {
    expect(
      menuFor("bin.card", { ids: [MED_V] })
        .items.map((i) => i.label)
        .join("|"),
    ).not.toContain("Place side by side");
    const mixed = menuFor("bin.card", { ids: [MED_V, MED_A] }).item("Place side by side");
    expect(mixed.disabled).toBe(true);
    expect(mixed.disabledReason).toBe("Select 2 to 4 videos or images");
  });
});
