// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Playback (WP-2H.10): the page sits between Editing and Media and import, its row is
// found by what people type, and the app's transport follows the preference: at the default, when
// it is turned on, and when it is turned off again.
import { defaultPreferences } from "@cutweave/preferences";
import { MockTransport, RpcClient } from "@cutweave/protocol";
import { afterEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n, type TKey } from "../i18n/index.ts";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import { playbackStore } from "../state/playbackStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { FLICKS } from "../testing/fixtures.ts";
import { transportController } from "../transport/useTransport.ts";
import "./core.ts";
import "./featurePages.tsx";
import { settingsPages } from "./registry.ts";
import { searchSettings } from "./search.ts";

const t = (key: TKey): string => i18n.t(key) as unknown as string;
const find = (query: string) =>
  searchSettings(settingsPages(), query, t).flatMap((m) => m.rows.map((r) => r.row.id));

describe("Settings > Playback", () => {
  it("is the page after Editing and before Media and import", () => {
    const ids = settingsPages().map((p) => p.id);
    const at = ids.indexOf("playback");
    expect(ids.slice(at - 1, at + 2)).toEqual(["editing", "playback", "media"]);
    const page = settingsPages()[at];
    expect(t(page?.title as TKey)).toBe("Playback");
    expect(page?.group).toBe("preferences");
  });

  it("lists its rows in the catalog's order, each on its preference", () => {
    const rows = settingsPages().find((p) => p.id === "playback")?.rows ?? [];
    expect(rows.map((r) => [r.key, r.control.kind, r.applies ?? "now"])).toEqual([
      ["playback.quality_playing", "segmented", "engine"],
      ["playback.quality_paused", "segmented", "engine"],
      ["playback.proxies", "switch", "engine"],
      ["playback.loop", "switch", "engine"],
      ["playback.return_to_start", "switch", "now"],
    ]);
    expect(defaultPreferences()).toMatchObject({
      "playback.quality_playing": "auto",
      "playback.quality_paused": "full",
      "playback.proxies": true,
      "playback.loop": false,
    });
  });

  it("finds each row by what people type", () => {
    expect(find("stutter")).toEqual(["playback.quality_playing"]);
    expect(find("quality while playing")).toEqual(["playback.quality_playing"]);
    expect(find("sharpens")).toEqual(["playback.quality_paused"]);
    expect(find("optimized media")).toEqual(["playback.proxies"]);
    expect(find("repeat")).toContain("playback.loop");
    expect(find("loop playback")).toEqual(["playback.loop"]);
  });

  it("has a switch for going back to where playback started, off by default", () => {
    const row = settingsPages()
      .find((p) => p.id === "playback")
      ?.rows.find((r) => r.id === "playback.return_to_start");
    expect(row?.key).toBe("playback.return_to_start");
    expect(row?.control.kind).toBe("switch");
    expect(defaultPreferences()["playback.return_to_start"]).toBe(false);
  });

  it("is found by its label, its description and its keywords", () => {
    expect(find("where playback started")).toEqual(["playback.return_to_start"]);
    expect(find("pressed play")).toEqual(["playback.return_to_start"]);
    expect(find("rewind")).toEqual(["playback.return_to_start"]);
    expect(find("playback")).toContain("playback.return_to_start");
  });
});

describe("the app's transport and playback.return_to_start", () => {
  const stopped = (t: number) => ({ state: "stopped", playing: false, rate: 0, t });
  const playing = (t: number) => ({ state: "playing", playing: true, rate: 1, t });

  afterEach(async () => {
    engineStore.setState({ rpc: null });
    await preferences.reset(["playback.return_to_start"]);
  });

  it("stays put at the default, returns once the setting is on, and stays put when it is off again", async () => {
    const mock = new MockTransport();
    engineStore.setState({ rpc: new RpcClient(mock), connection: "ready" });
    mock.handle("view.play", () => playing(FLICKS));
    mock.handle("view.pause", () => stopped(3 * FLICKS));
    mock.handle("view.seek", (p) => stopped((p as { t: number }).t));
    const seeks = () => mock.requests.filter((r) => r.method === "view.seek").map((r) => r.params);
    const transport = transportController();
    const off = transport.attach();
    try {
      const playAndPause = async () => {
        playbackStore.getState().applyState(stopped(FLICKS));
        await transport.play();
        await transport.pause();
      };

      await playAndPause();
      expect(seeks()).toEqual([]);
      expect(viewStore.getState().playhead).toBe(3 * FLICKS);

      await preferences.set("playback.return_to_start", true);
      expect(prefsStore.getState().values["playback.return_to_start"]).toBe(true);
      await playAndPause();
      await vi.waitFor(() => expect(seeks()).toEqual([{ t: FLICKS }]));
      await vi.waitFor(() => expect(playbackStore.getState().holdPlayhead).toBe(false));
      expect(viewStore.getState().playhead).toBe(FLICKS);

      await preferences.set("playback.return_to_start", false);
      await playAndPause();
      expect(seeks()).toEqual([{ t: FLICKS }]);
      expect(viewStore.getState().playhead).toBe(3 * FLICKS);
    } finally {
      off();
    }
  });
});
