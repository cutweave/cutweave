// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom

import { ToastProvider } from "@cutweave/ui";
import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { createToastStore, MAX_NOTICES, toastStore } from "../state/toastStore.ts";
import { Toasts } from "./Toasts.tsx";

beforeEach(() => toastStore.getState().clear());
afterEach(cleanup);

describe("toast store", () => {
  it("keeps the newest notices and replaces an identical one", () => {
    const store = createToastStore();
    const first = store.getState().push({ tone: "warning", title: "Clip not moved", message: "a" });
    store.getState().push({ tone: "warning", title: "Clip not moved", message: "a" });
    expect(store.getState().notices).toHaveLength(1);
    expect(store.getState().notices[0]?.id).not.toBe(first);
    for (let i = 0; i < 5; i++) store.getState().push({ tone: "info", title: `n${i}` });
    expect(store.getState().notices.map((n) => n.title)).toEqual(["n2", "n3", "n4"]);
    expect(MAX_NOTICES).toBe(3);
    store.getState().dismiss(store.getState().notices[0]?.id ?? 0);
    expect(store.getState().notices.map((n) => n.title)).toEqual(["n3", "n4"]);
  });
});

describe("Toasts", () => {
  it("shows each notice as a design-system toast and dismisses it", () => {
    render(
      h(
        I18nextProvider,
        { i18n },
        h(ToastProvider, { label: "Notifications", children: h(Toasts) }),
      ),
    );
    act(() => {
      toastStore.getState().push({
        tone: "warning",
        title: "Clip not moved",
        message: "The track is locked. Unlock it to edit its clips.",
      });
    });
    expect(screen.getByText("Clip not moved")).toBeTruthy();
    expect(screen.getByText("The track is locked. Unlock it to edit its clips.")).toBeTruthy();
    act(() => screen.getByRole("button", { name: "Dismiss" }).click());
    expect(toastStore.getState().notices).toEqual([]);
  });
});
