// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { packageName } from "./index.ts";

describe("@cutweave/desktop", () => {
  it("exports its package name", () => {
    expect(packageName).toBe("@cutweave/desktop");
  });
});
