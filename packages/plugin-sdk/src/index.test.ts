// SPDX-License-Identifier: MIT OR Apache-2.0
import { describe, expect, it } from "vitest";
import { packageName } from "./index.ts";

describe("@cutweave/plugin-sdk", () => {
  it("exports its package name", () => {
    expect(packageName).toBe("@cutweave/plugin-sdk");
  });
});
