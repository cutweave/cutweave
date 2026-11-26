// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.08: the recorded transcripts. Each replays an event stream through the machine (real
// hit-testing, the engine's snapping through the math module) and asserts the intents, the undo
// steps the fake engine committed and the final state.
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import { replay } from "./testing/transcript.ts";
import { TRANSCRIPTS } from "./testing/transcripts.ts";
import { World } from "./testing/world.ts";

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

describe("transcripts", () => {
  it("are at least 60, with unique names", () => {
    expect(TRANSCRIPTS.length).toBeGreaterThanOrEqual(60);
    expect(new Set(TRANSCRIPTS.map((t) => t.name)).size).toBe(TRANSCRIPTS.length);
  });

  for (const t of TRANSCRIPTS) {
    it(t.name, () => {
      const r = replay(t, (o) => new World(o));
      expect(r.shown).toEqual(t.expect);
      if (t.history !== undefined) expect(r.history).toEqual(t.history);
      expect(r.world.state.mode.name).toBe(t.end);
      // Whatever happened, the engine saw a well-formed protocol (the fake engine throws
      // otherwise) and no gesture is open unless the transcript ends inside one.
      const open = r.world.engine.open.size;
      expect(open).toBe("tx" in r.world.state.mode ? 1 : 0);
    });
  }
});
