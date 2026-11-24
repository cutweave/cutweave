// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { forcedPreviewPath } from "./preview-path.ts";

describe("forcedPreviewPath", () => {
  it("is jpeg only for CUTWEAVE_PREVIEW=jpeg (any case, trimmed)", () => {
    expect(forcedPreviewPath({ CUTWEAVE_PREVIEW: "jpeg" })).toBe("jpeg");
    expect(forcedPreviewPath({ CUTWEAVE_PREVIEW: " JPEG " })).toBe("jpeg");
    expect(forcedPreviewPath({ CUTWEAVE_PREVIEW: "gpu" })).toBeUndefined();
    expect(forcedPreviewPath({ CUTWEAVE_PREVIEW: "" })).toBeUndefined();
    expect(forcedPreviewPath({})).toBeUndefined();
  });
});
