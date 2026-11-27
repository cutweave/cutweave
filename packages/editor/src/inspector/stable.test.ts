// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { selectSelectedClips } from "./clips.ts";
import type { SelectedClip } from "./fields.ts";
import { insideKey, sameExceptStart } from "./stable.ts";

const clipsOf = (edits: Record<string, object> = {}): SelectedClip[] =>
  selectSelectedClips(sampleProject(edits), [{ kind: "item", id: "itm_v" } as never]);

describe("sameExceptStart", () => {
  it("ignores a moved start while the playhead stays inside", () => {
    expect(sameExceptStart(clipsOf(), clipsOf({ itm_v: { start: sec(1.5) } }), sec(2))).toBe(true);
  });

  it("tells when the playhead moves in or out of the clip", () => {
    const moved = clipsOf({ itm_v: { start: sec(3) } });
    expect(sameExceptStart(clipsOf(), moved, sec(2))).toBe(false);
    expect(insideKey(clipsOf(), sec(2))).toBe("1");
    expect(insideKey(moved, sec(2))).toBe("0");
  });

  it("follows any other change of the clip", () => {
    expect(sameExceptStart(clipsOf(), clipsOf({ itm_v: { duration: sec(3) } }), sec(2))).toBe(
      false,
    );
    expect(sameExceptStart(clipsOf(), clipsOf({ itm_v: { enabled: false } }), sec(2))).toBe(false);
  });

  it("follows the start of a clip with keyframes", () => {
    const keyed = { visual: { opacity: { keyframes: [{ t: 0, v: 1 }] } } };
    expect(
      sameExceptStart(
        clipsOf({ itm_v: keyed }),
        clipsOf({ itm_v: { ...keyed, start: sec(1.5) } }),
        sec(2),
      ),
    ).toBe(false);
  });
});
