// SPDX-License-Identifier: MIT OR Apache-2.0
import { describe, expect, it } from "vitest";
import { packageName } from "./index.ts";

describe("@cutweave/compose-runtime", () => {
  it("exports its package name", () => {
    expect(packageName).toBe("@cutweave/compose-runtime");
  });
});
