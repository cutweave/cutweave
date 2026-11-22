// SPDX-License-Identifier: MIT
import { describe, expect, it } from "vitest";
import { packageName } from "./index.ts";

describe("@cutweave/plugin-sdk", () => {
  it("exports its package name", () => {
    expect(packageName).toBe("@cutweave/plugin-sdk");
  });
});
