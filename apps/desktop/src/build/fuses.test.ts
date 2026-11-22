// SPDX-License-Identifier: GPL-3.0-or-later
import { FuseV1Options, FuseVersion } from "@electron/fuses";
import { describe, expect, it } from "vitest";
import { FUSES, fuseConfig, parseFuseReadOutput } from "./fuses.ts";

describe("fuses", () => {
  it("covers every fuse in the v1 wire (strict mode)", () => {
    const wire = Object.keys(FuseV1Options).filter((k) => Number.isNaN(Number(k)));
    expect(Object.keys(FUSES).sort()).toEqual(wire.sort());
  });

  it("disables the Node entry points and requires the integrity-checked asar", () => {
    expect(FUSES).toMatchObject({
      RunAsNode: false,
      EnableNodeOptionsEnvironmentVariable: false,
      EnableNodeCliInspectArguments: false,
      EnableCookieEncryption: true,
      EnableEmbeddedAsarIntegrityValidation: true,
      OnlyLoadAppFromAsar: true,
      GrantFileProtocolExtraPrivileges: false,
    });
  });

  it("maps names to wire indices for @electron/fuses", () => {
    const config = fuseConfig("win32", "x64") as unknown as Record<string | number, unknown>;
    expect(config.version).toBe(FuseVersion.V1);
    expect(config.strictlyRequireAllFuses).toBe(true);
    expect(config.resetAdHocDarwinSignature).toBe(false);
    expect(config[FuseV1Options.RunAsNode]).toBe(false);
    expect(config[FuseV1Options.OnlyLoadAppFromAsar]).toBe(true);
    expect(
      (fuseConfig("darwin", "arm64") as { resetAdHocDarwinSignature: boolean })
        .resetAdHocDarwinSignature,
    ).toBe(true);
  });

  it("parses `electron-fuses read` output, colors included", () => {
    const out = [
      "Analyzing app: \u001b[36melectron.exe\u001b[39m",
      "Fuse Version: \u001b[36mv1\u001b[39m",
      "  \u001b[33mRunAsNode\u001b[39m is \u001b[31mDisabled\u001b[39m",
      "  EnableCookieEncryption is Enabled",
      "  WasmTrapHandlers is Inherited",
    ].join("\n");
    expect(parseFuseReadOutput(out)).toEqual({
      RunAsNode: false,
      EnableCookieEncryption: true,
      WasmTrapHandlers: null,
    });
  });
});
