// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// `useUnsavedWork` (the dot by Save and in the window title, asked by the app shell) follows the
// answer, not the document: a drag changes the document every frame, and the shell drawing again
// with it drew the whole window again on every update (issue #241).
import type { DocPatched, PatchOp, Project } from "@cutweave/protocol";
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { docStore } from "../state/docStore.ts";
import { sessionStore } from "./sessionStore.ts";
import { useUnsavedWork } from "./useProject.ts";

const PROJECT = {
  meta: { name: "P" },
  settings: {},
  media: {},
  sequences: {
    seq_1: {
      id: "seq_1",
      name: "Main",
      format: { width: 1920, height: 1080, frame_rate: { num: 30, den: 1 } },
      track_order: ["trk_v"],
      markers: {},
      in_out: null,
      tracks: {
        trk_v: {
          id: "trk_v",
          kind: "video",
          name: "V1",
          items: { itm_1: { kind: "clip", id: "itm_1", start: 0, duration: 1_000 } },
          muted: false,
          hidden: false,
          locked: false,
        },
      },
    },
  },
} as unknown as Project;

/** A drag update: the clip moved from `old` to `start`. */
const moved = (rev: number, old: number, start: number): DocPatched =>
  ({
    rev,
    prev_rev: rev - 1,
    committed_rev: 1,
    actor: "user",
    tx_id: "tx_drag",
    cause: "command",
    request_id: null,
    ops: [
      {
        op: "replace",
        path: "/sequences/seq_1/tracks/trk_v/items/itm_1/start",
        old,
        value: start,
      },
    ] as unknown as PatchOp[],
  }) as unknown as DocPatched;

beforeEach(() => {
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  docStore.getState().resync({ rev: 1, committed_rev: 1, project: PROJECT });
  sessionStore.setState({ known: true, path: "/work/P", dirty: false });
});
afterEach(cleanup);

describe("useUnsavedWork", () => {
  it("does not render again while a drag changes the document, only when the answer changes", () => {
    let renders = 0;
    const { result } = renderHook(() => {
      renders += 1;
      return useUnsavedWork();
    });
    expect(result.current).toBe(false);
    const before = renders;
    act(() => {
      let at = 0;
      for (let rev = 2; rev <= 6; rev += 1) {
        expect(docStore.getState().applyEvent(moved(rev, at, rev * 100))).toBe("applied");
        at = rev * 100;
      }
    });
    expect(renders).toBe(before);
    act(() => sessionStore.getState().setDirty(true));
    expect(result.current).toBe(true);
    expect(renders).toBe(before + 1);
  });

  it("counts an untitled project with content as unsaved work", () => {
    sessionStore.setState({ known: true, path: null, dirty: false });
    const { result } = renderHook(() => useUnsavedWork());
    expect(result.current).toBe(true);
  });
});
