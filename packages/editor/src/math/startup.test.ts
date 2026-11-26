// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { createElement } from "react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import { createI18n } from "../i18n/index.ts";
import { MathStartupError } from "./MathStartupError.tsx";
import { startMath } from "./startup.ts";

// The smallest valid module: no exports, so it cannot match the engine's.
const EMPTY_MODULE = new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]);

describe("startMath", () => {
  it("reports a module that does not match this build as a version failure", async () => {
    const result = await startMath(async () => ({ default: EMPTY_MODULE }));
    expect(result).toMatchObject({ ok: false, kind: "version" });
  });

  it("reports a module that cannot load as a load failure", async () => {
    const result = await startMath(async () => {
      throw new Error("chunk failed");
    });
    expect(result).toEqual({ ok: false, kind: "load", detail: "chunk failed" });
  });
});

describe("MathStartupError", () => {
  it("is a blocking alert with the failure and what to do", () => {
    render(
      createElement(
        I18nextProvider,
        { i18n: createI18n() },
        createElement(MathStartupError, {
          failure: { ok: false, kind: "version", detail: "version 0 expected 1" },
        }),
      ),
    );
    expect(screen.getByRole("alert").textContent).toContain("math module is out of date");
    expect(screen.getByText("version 0 expected 1")).toBeTruthy();
  });
});
