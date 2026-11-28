// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Settings > Editing and Settings > Timeline (WP-2A.13): the rows show the catalog's defaults,
// write through the preferences store, and what the timeline makes of them (`timeline/prefs.ts`).
import { defaultPreferences, type PreferenceMap } from "@cutweave/preferences";
import { TooltipProvider } from "@cutweave/ui";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import { SettingsDialog } from "../shell/SettingsDialog.tsx";
import {
  displayTime,
  fieldTime,
  machinePrefs,
  newTrackHeight,
  snapKindsOf,
  timelineDisplay,
} from "../timeline/prefs.ts";
import { settingsPages } from "./registry.ts";
import { settingsStore } from "./settingsStore.ts";
import { EDITING_ROWS, TIMELINE_PAGE } from "./timelinePages.tsx";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(async () => {
  await preferences.attach(null);
  prefsStore.setState({ values: defaultPreferences() });
  engineStore.setState({ ...initialEngineState, connection: "ready" });
  settingsStore.getState().setOpen(true);
});

afterEach(() => {
  cleanup();
  settingsStore.getState().setOpen(false);
  prefsStore.setState({ values: defaultPreferences() });
  engineStore.setState({ ...initialEngineState });
});

const open = async (page: string) => {
  const user = userEvent.setup();
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SettingsDialog))));
  const settings = await screen.findByRole("dialog", { name: "Settings" });
  await user.click(within(settings).getByRole("tab", { name: page }));
  return { user, settings };
};
const radios = (group: HTMLElement) =>
  within(group)
    .getAllByRole("radio")
    .map((r) => `${r.textContent}:${r.getAttribute("aria-checked")}`);
const value = <K extends keyof PreferenceMap>(key: K): PreferenceMap[K] =>
  prefsStore.getState().values[key];

describe("the pages", () => {
  it("puts Timeline right after Editing, and the new rows after the Editing page's own", () => {
    const ids = settingsPages().map((p) => p.id);
    expect(ids.indexOf("timeline")).toBe(ids.indexOf("editing") + 1);
    const editing = settingsPages().find((p) => p.id === "editing");
    const rows = editing?.rows.map((r) => r.id) ?? [];
    expect(rows[0]).toBe("editing.still_duration");
    for (const row of EDITING_ROWS) expect(rows, row.id).toContain(row.id);
    expect(TIMELINE_PAGE.rows.map((r) => r.id)).toEqual([
      "timeline.snap",
      "timeline.snap_targets",
      "timeline.snap_distance",
      "timeline.follow",
      "timeline.magnetic_main",
      "timeline.thumbnails",
      "timeline.waveforms",
      "timeline.track_height",
      "timeline.wheel",
      "timeline.time_display",
      "timeline.drop_frame",
    ]);
  });
});

describe("Settings > Editing", () => {
  it("shows the defaults and writes each choice through the store", async () => {
    const { user, settings } = await open("Editing");
    const link = within(settings).getByRole("switch", { name: "Keep video and audio linked" });
    expect(link.getAttribute("aria-checked")).toBe("true");
    const drop = within(settings).getByRole("radiogroup", { name: "Dropping media onto a clip" });
    expect(radios(drop)).toEqual(["Overwrite:true", "Insert:false"]);
    const nudge = within(settings).getByRole("spinbutton", { name: "Nudge step" });
    expect((nudge as HTMLInputElement).value).toBe("1");
    const undo = within(settings).getByRole("spinbutton", { name: "Undo steps" });
    expect((undo as HTMLInputElement).value).toBe("1000");

    await user.click(link);
    await user.click(within(drop).getByRole("radio", { name: "Insert" }));
    await user.clear(undo);
    await user.type(undo, "2");
    await waitFor(() => expect(value("editing.undo_steps")).toBe(2));
    expect(value("editing.link_av")).toBe(false);
    expect(value("editing.drop_mode")).toBe("insert");
    // An empty or out-of-range number is never stored.
    await user.clear(nudge);
    await user.type(nudge, "500");
    expect(value("editing.nudge")).toBe(50);
  });
});

describe("Settings > Timeline", () => {
  it("shows the defaults and writes each choice through the store", async () => {
    const { user, settings } = await open("Timeline");
    const sw = (name: string) => within(settings).getByRole("switch", { name });
    for (const name of [
      "Snapping on when a project opens",
      "Playhead",
      "Clip edges",
      "Markers",
      "Beats",
      "Audio waveforms on clips",
      "Drop-frame timecode",
    ]) {
      expect(sw(name).getAttribute("aria-checked"), name).toBe("true");
    }
    const group = (name: string) => within(settings).getByRole("radiogroup", { name });
    expect(radios(group("Follow the playhead while playing"))).toEqual([
      "Page:true",
      "Smooth:false",
      "Off:false",
    ]);
    expect(radios(group("Video clip thumbnails"))).toEqual([
      "Filmstrip:true",
      "First frame:false",
      "Off:false",
    ]);
    expect(radios(group("Height of new tracks"))).toEqual([
      "Small:false",
      "Medium:true",
      "Large:false",
    ]);
    expect(radios(group("Mouse wheel"))).toEqual(["Scrolls:true", "Zooms:false"]);
    expect(radios(group("Show time as"))).toEqual([
      "Timecode:true",
      "Frames:false",
      "Seconds:false",
    ]);

    await user.click(sw("Snapping on when a project opens"));
    await user.click(sw("Beats"));
    await user.click(sw("Playhead"));
    await user.click(
      within(group("Follow the playhead while playing")).getByRole("radio", { name: "Off" }),
    );
    await user.click(within(group("Height of new tracks")).getByRole("radio", { name: "Large" }));
    await user.click(within(group("Mouse wheel")).getByRole("radio", { name: "Zooms" }));
    await user.click(within(group("Show time as")).getByRole("radio", { name: "Frames" }));
    await waitFor(() => expect(value("timeline.time_display")).toBe("frames"));
    expect(value("timeline.snap")).toBe(false);
    expect(value("timeline.snap_targets")).toEqual(["edges", "markers"]);
    expect(value("timeline.follow")).toBe("off");
    expect(value("timeline.track_height")).toBe("large");
    expect(value("timeline.wheel")).toBe("zoom");
    // Drop-frame is a property of timecode: off the table while time shows as frames.
    await waitFor(() => expect(sw("Drop-frame timecode").hasAttribute("disabled")).toBe(true));
  });

  it("has no axe violations on either page", async () => {
    const { user, settings } = await open("Timeline");
    for (const page of ["Timeline", "Editing"]) {
      await user.click(within(settings).getByRole("tab", { name: page }));
      const results = await axe.run(document.body, {
        rules: { "color-contrast": { enabled: false } },
      });
      expect(
        results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`),
        page,
      ).toEqual([]);
    }
  }, 30_000);
});

describe("what the timeline makes of the values", () => {
  const defaults = defaultPreferences();

  it("gives the machine the snap distance, the snap kinds, the drop mode and the nudge step", () => {
    expect(machinePrefs(defaults)).toEqual({
      snapThresholdPx: 8,
      snapKinds: ["playhead", "edge", "marker", "in_out", "beat"],
      dropOnClip: "overwrite",
      nudgeFrames: 1,
    });
    expect(
      machinePrefs({
        ...defaults,
        "timeline.snap_distance": 16,
        "timeline.snap_targets": ["edges"],
        "editing.drop_mode": "insert",
        "editing.nudge": 5,
      }),
    ).toEqual({ snapThresholdPx: 16, snapKinds: ["edge"], dropOnClip: "insert", nudgeFrames: 5 });
    expect(snapKindsOf([])).toEqual([]);
  });

  it("gives the canvas its display", () => {
    expect(timelineDisplay(defaults)).toEqual({
      timeFormat: "timecode",
      dropFrame: true,
      thumbnails: "filmstrip",
      waveforms: true,
      wheel: "scroll",
    });
  });

  it("shows a time as timecode, frames or seconds, and in a field in a form it reads back", () => {
    const fps = { num: 24, den: 1 };
    const t = 2 * 705_600_000;
    const as = (format: PreferenceMap["timeline.time_display"]) => ({
      "timeline.time_display": format,
      "timeline.drop_frame": true,
    });
    expect(displayTime(t, fps, as("timecode"))).toBe("00:00:02:00");
    expect(displayTime(t, fps, as("frames"))).toBe("48");
    expect(displayTime(t, fps, as("seconds"))).toBe("2s");
    expect(fieldTime(t, fps, as("frames"))).toBe("f:48");
    expect(fieldTime(t, fps, as("seconds"))).toBe("2s");
    const ntsc = { num: 30000, den: 1001 };
    expect(displayTime(61 * 705_600_000, ntsc, as("timecode"))).toContain(";");
    expect(
      displayTime(61 * 705_600_000, ntsc, { ...as("timecode"), "timeline.drop_frame": false }),
    ).not.toContain(";");
  });

  it("reads the height of a new track from the design tokens; medium sends nothing", () => {
    const el = document.createElement("div");
    document.body.append(el);
    el.style.setProperty("--track-sm", "32px");
    el.style.setProperty("--track-lg", "72px");
    expect(newTrackHeight("small", el)).toBe(32);
    expect(newTrackHeight("large", el)).toBe(72);
    expect(newTrackHeight("medium", el)).toBeNull();
    expect(newTrackHeight("large", null)).toBeNull();
    el.remove();
  });
});
