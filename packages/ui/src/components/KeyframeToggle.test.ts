// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { KeyframeGroup, KeyframeToggle } from "./KeyframeToggle.tsx";

afterEach(cleanup);

describe("KeyframeToggle", () => {
  it.each([
    ["off", "Add keyframe", "false", "keyframe-diamond"],
    ["on", "Remove keyframe at playhead", "true", "keyframe-diamond-filled"],
    ["elsewhere", "Add keyframe at playhead", "false", "keyframe-diamond"],
  ] as const)("state %s: label, aria-pressed and glyph", (state, name, pressed, glyph) => {
    render(h(KeyframeToggle, { state, onToggle: () => {} }));
    const button = screen.getByRole("button", { name });
    expect(button.getAttribute("aria-pressed")).toBe(pressed);
    expect(button.getAttribute("data-state")).toBe(state);
    expect(button.querySelector("svg")?.getAttribute("data-icon")).toBe(glyph);
  });

  it("names the property and toggles with Space and Enter", async () => {
    const onToggle = vi.fn();
    const user = userEvent.setup();
    render(h(KeyframeToggle, { state: "off", property: "Opacity", onToggle }));
    await user.tab();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Add keyframe for Opacity" }),
    );
    await user.keyboard(" ");
    await user.keyboard("{Enter}");
    expect(onToggle).toHaveBeenCalledTimes(2);
  });

  it("is disabled and labelled as unavailable when the property cannot animate", async () => {
    const onToggle = vi.fn();
    const user = userEvent.setup();
    render(h(KeyframeToggle, { state: "off", disabled: true, onToggle }));
    await user.click(screen.getByRole("button", { name: "Keyframes unavailable" }));
    expect(onToggle).not.toHaveBeenCalled();
  });
});

describe("KeyframeGroup", () => {
  it("renders prev / toggle / next and routes each handler", async () => {
    const onPrev = vi.fn();
    const onNext = vi.fn();
    const onToggle = vi.fn();
    const user = userEvent.setup();
    render(h(KeyframeGroup, { state: "on", property: "Position X", onPrev, onNext, onToggle }));
    expect(screen.getByRole("group", { name: "Keyframes for Position X" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /Previous keyframe/ }));
    await user.click(screen.getByRole("button", { name: /Next keyframe/ }));
    await user.click(screen.getByRole("button", { name: /Remove keyframe/ }));
    expect(onPrev).toHaveBeenCalledOnce();
    expect(onNext).toHaveBeenCalledOnce();
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it("disables prev/next individually", () => {
    render(
      h(KeyframeGroup, {
        state: "elsewhere",
        onPrev: () => {},
        onNext: () => {},
        prevDisabled: true,
        onToggle: () => {},
      }),
    );
    expect((screen.getByRole("button", { name: /Previous/ }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect((screen.getByRole("button", { name: /Next/ }) as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
});
