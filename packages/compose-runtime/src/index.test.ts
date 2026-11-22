// SPDX-License-Identifier: MIT
import { describe, expect, it } from "vitest";
import { packageName } from "./index.ts";

describe("@cutweave/compose-runtime", () => {
  it("exports its package name", () => {
    expect(packageName).toBe("@cutweave/compose-runtime");
  });
});
