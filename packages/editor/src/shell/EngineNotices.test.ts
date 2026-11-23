// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { ToastProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { EngineNotices, EngineStatusChip } from "./EngineNotices.tsx";

function mount() {
  return render(
    h(I18nextProvider, { i18n }, h(ToastProvider, null, h(EngineStatusChip), h(EngineNotices))),
  );
}

beforeEach(() => engineStore.setState({ ...initialEngineState }));
afterEach(cleanup);

describe("engine status chip", () => {
  it("follows the connection, the supervisor and any block", () => {
    mount();
    const chip = () => document.querySelector(".cw-engine-status")?.textContent;
    expect(chip()).toBe("Engine not connected");
    act(() => engineStore.setState({ connection: "ready" }));
    expect(chip()).toBe("Engine connected");
    act(() => engineStore.setState({ connection: "reconnecting" }));
    expect(chip()).toBe("Reconnecting");
    act(() => engineStore.setState({ blocked: { kind: "engine-failed", message: null } }));
    expect(chip()).toBe("Engine stopped");
    act(() => engineStore.setState({ blocked: { kind: "schema-mismatch", engineVersion: "9" } }));
    expect(chip()).toBe("Engine mismatch");
  });
});

describe("engine notices", () => {
  it("says what the restart recovered", async () => {
    mount();
    act(() =>
      engineStore.setState({
        notice: {
          id: 1,
          recovery: { project: "/w/film/film.cutweave", applied: 3, damaged: false },
        },
      }),
    );
    expect(await screen.findByText("Engine restarted — recovered")).toBeTruthy();
    expect(screen.getByText("3 unsaved changes restored from film.cutweave.")).toBeTruthy();
  });

  it("warns that an untitled project was not kept", async () => {
    mount();
    act(() => engineStore.setState({ notice: { id: 2, recovery: null } }));
    expect(await screen.findByText("Engine restarted")).toBeTruthy();
    expect(screen.getByText(/untitled project were not kept/)).toBeTruthy();
  });

  it("blocks editing with a message while the engine failed or mismatches", async () => {
    mount();
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => engineStore.setState({ blocked: { kind: "engine-failed", message: null } }));
    const dialog = await screen.findByRole("dialog", { name: "The engine stopped" });
    expect(dialog.textContent).toContain("Editing is paused");
    const results = await axe.run(dialog);
    expect(results.violations.map((v) => v.id)).toEqual([]);
    act(() =>
      engineStore.setState({ blocked: { kind: "schema-mismatch", engineVersion: "9.9.9" } }),
    );
    expect(
      await screen.findByRole("dialog", { name: "This editor does not match the engine" }),
    ).toBeTruthy();
    expect(screen.getByRole("dialog").textContent).toContain("Engine 9.9.9");
    act(() => engineStore.setState({ blocked: null }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
