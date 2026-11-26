// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2G.04: Settings > Export's default preset and file name pattern.
import { defaultPreferences } from "@cutweave/preferences";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import type { SettingsContext } from "../settings/registry.ts";
import { PRESET_ENTRIES } from "../testing/exportFixtures.ts";
import { DefaultPresetControl, FileNameControl } from "./ExportSettingRows.tsx";
import { exportStore, initialExportState } from "./exportStore.ts";
import { resetExportController } from "./useExport.ts";

function context(values: Partial<ReturnType<typeof defaultPreferences>> = {}) {
  const set = vi.fn(async () => undefined);
  const ctx = {
    client: { set } as never,
    values: { ...defaultPreferences(), ...values },
    desktop: null,
    t: (key: string, vars?: Record<string, unknown>) => i18n.t(key as never, vars as never),
    say: () => undefined,
    confirm: async () => true,
  } as unknown as SettingsContext;
  return { ctx, set };
}

beforeEach(() => {
  resetExportController();
  exportStore.setState({ ...initialExportState });
  const call = vi.fn(async () => ({ presets: PRESET_ENTRIES, problems: [] }));
  engineStore.setState({ ...initialEngineState, rpc: { call } as never, connection: "ready" });
});

afterEach(() => {
  cleanup();
  resetExportController();
});

describe("Settings > Export", () => {
  it("offers the last one used and the engine's presets, grouped, without the unsupported ones", async () => {
    const { ctx, set } = context();
    render(h(DefaultPresetControl, { ctx, id: "p" }));
    const select = (await screen.findByRole("combobox", {
      name: "Default preset",
    })) as HTMLSelectElement;
    await screen.findByRole("option", { name: "TikTok" });
    expect(select.value).toBe("");
    expect(screen.getByRole("option", { name: "The last one used" })).toBeTruthy();
    expect(screen.queryByRole("option", { name: "GIF" })).toBeNull();
    await userEvent.selectOptions(select, "tiktok");
    expect(set).toHaveBeenCalledWith("export.preset", "tiktok");
  });

  it("saves a valid file name pattern as it is typed and says what it makes", async () => {
    const { ctx, set } = context();
    render(h(FileNameControl, { ctx, id: "f" }));
    const field = screen.getByRole("textbox", { name: "File name" });
    expect(screen.getByText("Makes My project.mp4")).toBeTruthy();
    await userEvent.clear(field);
    await userEvent.type(field, "{{project}_{{preset}");
    expect(screen.getByText("Makes My project_TikTok.mp4")).toBeTruthy();
    expect(set).toHaveBeenLastCalledWith("export.filename", "{project}_{preset}");
    await userEvent.type(field, "/x");
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(set).toHaveBeenLastCalledWith("export.filename", "{project}_{preset}");
  });
});
