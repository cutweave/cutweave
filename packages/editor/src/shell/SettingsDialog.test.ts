// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// File > Settings… > Saving (WP-1M.10): the autosave preference lives in the view store.
import { TooltipProvider } from "@cutweave/ui";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { DEFAULT_AUTOSAVE, viewStore } from "../state/viewStore.ts";
import { settingsStore } from "./appCommands.ts";
import { SettingsDialog } from "./SettingsDialog.tsx";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  viewStore.getState().setAutosave(DEFAULT_AUTOSAVE);
  settingsStore.getState().setOpen(true);
});

afterEach(() => {
  cleanup();
  settingsStore.getState().setOpen(false);
});

const open = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SettingsDialog))));

const choices = (group: HTMLElement) =>
  within(group)
    .getAllByRole("radio")
    .map((r) => `${r.textContent}:${r.getAttribute("aria-checked")}`);

describe("Settings > Saving", () => {
  it("shows the defaults: automatic, every 30 s, 10 backups", async () => {
    open();
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    const automatic = within(dialog).getByRole("switch", { name: "Save automatically" });
    expect(automatic.getAttribute("aria-checked")).toBe("true");
    expect(
      choices(within(dialog).getByRole("radiogroup", { name: "Save at least every" })),
    ).toEqual(["30 s:true", "1 min:false", "2 min:false", "5 min:false"]);
    expect(choices(within(dialog).getByRole("radiogroup", { name: "Backups to keep" }))).toEqual([
      "5:false",
      "10:true",
      "20:false",
    ]);
  });

  it("changes the preference at once, and the choices follow the switch", async () => {
    open();
    const user = userEvent.setup();
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(dialog).getByRole("radio", { name: "2 min" }));
    await user.click(within(dialog).getByRole("radio", { name: "20" }));
    expect(viewStore.getState().autosave).toEqual({
      enabled: true,
      intervalS: 120,
      keepBackups: 20,
    });
    await user.click(within(dialog).getByRole("switch", { name: "Save automatically" }));
    expect(viewStore.getState().autosave.enabled).toBe(false);
    expect(within(dialog).getByRole("radio", { name: "5 min" }).hasAttribute("disabled")).toBe(
      true,
    );
  });
});
