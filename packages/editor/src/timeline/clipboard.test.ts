// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.06 (Vitest part): the clipboard module's own rules; the menu rows are tested with the
// surfaces and the whole path (real keys, the Edit menu, the engine) by flow F42.
import { afterEach, describe, expect, it, vi } from "vitest";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import {
  clipboardStore,
  copyClips,
  cutClips,
  hasClipboardItems,
  parsePayload,
  selectedClips,
} from "./clipboard.ts";

const payload = {
  version: 1,
  sequence: "seq_1",
  frame_rate: { num: 30, den: 1 },
  span: 1,
  tracks: [{ kind: "video", row: 0, source: "trk_1" }],
  items: [{ track: 0, item: { id: "itm_1" } }],
};

afterEach(() => {
  clipboardStore.getState().setPayload(null);
  viewStore.setState({ ...initialViewState });
});

describe("the clipboard store", () => {
  it("reads a payload back from text, and refuses anything else", () => {
    expect(parsePayload(JSON.stringify(payload))?.items).toHaveLength(1);
    expect(parsePayload("")).toBeNull();
    expect(parsePayload("hello")).toBeNull();
    expect(parsePayload(JSON.stringify({ ...payload, version: 2 }))).toBeNull();
    expect(parsePayload(JSON.stringify({ ...payload, items: [] }))).toBeNull();
    expect(parsePayload(JSON.stringify([1, 2]))).toBeNull();
  });

  it("says whether there is anything to paste", () => {
    expect(hasClipboardItems()).toBe(false);
    clipboardStore.getState().setPayload(payload as never);
    expect(hasClipboardItems()).toBe(true);
  });

  it("lists the selected clips and nothing else", () => {
    viewStore.getState().setSelection([
      { kind: "track", id: "trk_1" },
      { kind: "item", id: "itm_2" },
      { kind: "item", id: "itm_3" },
    ] as never);
    expect(selectedClips()).toEqual(["itm_2", "itm_3"]);
  });
});

describe("copy and cut", () => {
  it("copy asks the engine and keeps what it returns", async () => {
    const call = vi.fn(async () => ({ clipboard: payload }));
    await copyClips(call, ["itm_1"]);
    expect(call).toHaveBeenCalledWith("timeline.copy", { items: ["itm_1"] });
    expect(clipboardStore.getState().payload).toEqual(payload);
  });

  it("cut copies first and then makes the one mutation, item.delete", async () => {
    const call = vi.fn(async (name: string) =>
      name === "timeline.copy" ? { clipboard: payload } : {},
    );
    await cutClips(call, ["itm_1", "itm_2"]);
    expect(call.mock.calls.map((c) => c[0])).toEqual(["timeline.copy", "item.delete"]);
  });

  it("a cut whose copy fails deletes nothing", async () => {
    const call = vi.fn(async () => {
      throw new Error("refused");
    });
    await expect(cutClips(call, ["itm_1"])).rejects.toThrow("refused");
    expect(call).toHaveBeenCalledTimes(1);
  });
});
