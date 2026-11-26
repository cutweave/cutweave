// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NumberScrubber, type NumberScrubberProps } from "./NumberScrubber.tsx";

afterEach(cleanup);

type Props = Partial<NumberScrubberProps> & { initial?: number };

/** Controlled wrapper, like an inspector row bound to engine state. */
function Harness({ initial = 100, ...props }: Props) {
  const [value, setValue] = useState(initial);
  return h(NumberScrubber, {
    label: "Scale",
    value,
    ...props,
    onChange: (v: number) => {
      setValue(v);
      props.onChange?.(v);
    },
  });
}

const spin = () => screen.getByRole("spinbutton", { name: "Scale" });

/** Drags by pointer moves; each entry is the absolute clientX, modifiers optional. */
function drag(el: HTMLElement, xs: number[], mods: { shiftKey?: boolean; altKey?: boolean } = {}) {
  const [first, ...rest] = xs;
  fireEvent.pointerDown(el, { clientX: first, button: 0, pointerId: 1 });
  for (const x of rest) fireEvent.pointerMove(el, { clientX: x, pointerId: 1, ...mods });
}

describe("NumberScrubber: semantics", () => {
  it("is a spinbutton with value, range and unit text", () => {
    render(h(Harness, { initial: 25, min: 0, max: 400, unit: "%" }));
    const el = spin();
    expect(el.getAttribute("aria-valuenow")).toBe("25");
    expect(el.getAttribute("aria-valuemin")).toBe("0");
    expect(el.getAttribute("aria-valuemax")).toBe("400");
    expect(el.getAttribute("aria-valuetext")).toBe("25 %");
    expect(el.textContent).toContain("%");
    expect(el.tabIndex).toBe(0);
  });

  it("shows an em dash and announces mixed, without a numeric value", () => {
    render(h(NumberScrubber, { label: "Scale", value: 100, mixed: true, onChange: () => {} }));
    const el = spin();
    expect(el.getAttribute("data-state")).toBe("mixed");
    expect(el.getAttribute("aria-valuetext")).toBe("mixed");
    expect(el.hasAttribute("aria-valuenow")).toBe(false);
    expect(el.textContent).toContain("—");
  });

  it("is aria-disabled and ignores keys and drags when disabled", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { disabled: true, onChange }));
    const el = spin();
    expect(el.getAttribute("aria-disabled")).toBe("true");
    await user.tab();
    expect(document.activeElement).toBe(document.body);
    drag(el, [0, 40]);
    el.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("NumberScrubber: keyboard stepping", () => {
  it("steps with Up/Down and commits once per key press", async () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange, onCommit }));
    await user.tab();
    await user.keyboard("{ArrowUp}");
    expect(onChange).toHaveBeenLastCalledWith(101);
    expect(onCommit).toHaveBeenLastCalledWith(101);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(onChange).toHaveBeenLastCalledWith(99);
    expect(onCommit).toHaveBeenCalledTimes(3);
  });

  it("Shift steps x10 and Alt steps x0.1", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.tab();
    await user.keyboard("{Shift>}{ArrowUp}{/Shift}");
    expect(onChange).toHaveBeenLastCalledWith(110);
    await user.keyboard("{Alt>}{ArrowUp}{/Alt}");
    expect(onChange).toHaveBeenLastCalledWith(110.1);
    await user.keyboard("{Alt>}{ArrowDown}{ArrowDown}{/Alt}");
    expect(onChange).toHaveBeenLastCalledWith(109.9);
  });

  it("keeps integer steps exact over many presses", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: 0, onChange }));
    await user.tab();
    await user.keyboard("{ArrowUp>50/}");
    for (const [v] of onChange.mock.calls) expect(Number.isInteger(v)).toBe(true);
    expect(onChange).toHaveBeenLastCalledWith(50);
  });

  it("keeps fractional steps free of float drift", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: 0, step: 0.1, onChange }));
    await user.tab();
    await user.keyboard("{ArrowUp>3/}");
    expect(onChange.mock.calls.map(([v]) => v)).toEqual([0.1, 0.2, 0.3]);
  });

  it("clamps at min and max and stays silent at the bound", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: 99, min: 0, max: 100, onChange }));
    await user.tab();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(onChange).toHaveBeenCalledExactlyOnceWith(100);
  });
});

describe("NumberScrubber: drag to scrub", () => {
  it("scrubs by sensitivity and commits exactly once on release", () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    render(h(Harness, { initial: 0, sensitivity: 4, onChange, onCommit }));
    const el = spin();
    drag(el, [100, 104, 108, 112, 128]);
    // First move passes the threshold only; the rest scrub 1, 1, 4 steps.
    expect(onChange.mock.calls.map(([v]) => v)).toEqual([1, 2, 6]);
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(onCommit).toHaveBeenCalledExactlyOnceWith(6);
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("scrubs negative with leftward movement", () => {
    const onChange = vi.fn();
    render(h(Harness, { initial: 10, sensitivity: 4, onChange }));
    drag(spin(), [100, 90, 86, 78]);
    expect(onChange.mock.calls.map(([v]) => v)).toEqual([9, 7]);
  });

  it("applies Shift x10 and Alt x0.1 while dragging, exactly", () => {
    const shiftChange = vi.fn();
    render(h(Harness, { initial: 0, sensitivity: 4, onChange: shiftChange }));
    drag(spin(), [0, 10, 14], { shiftKey: true });
    expect(shiftChange.mock.calls.map(([v]) => v)).toEqual([10]);
    cleanup();

    const altChange = vi.fn();
    render(h(Harness, { initial: 0, sensitivity: 4, onChange: altChange }));
    drag(spin(), [0, 10, 14, 18, 22], { altKey: true });
    expect(altChange.mock.calls.map(([v]) => v)).toEqual([0.1, 0.2, 0.3]);
  });

  it("emits integers only for integer steps", () => {
    const onChange = vi.fn();
    render(h(Harness, { initial: 0, sensitivity: 3, onChange }));
    const xs = Array.from({ length: 80 }, (_, i) => i * 2);
    drag(spin(), xs);
    expect(onChange).toHaveBeenCalled();
    for (const [v] of onChange.mock.calls) expect(Number.isInteger(v)).toBe(true);
  });

  it("respects min and max while dragging", () => {
    const onChange = vi.fn();
    render(h(Harness, { initial: 98, max: 100, sensitivity: 2, onChange }));
    drag(spin(), [0, 10, 20, 30, 40]);
    expect(Math.max(...onChange.mock.calls.map(([v]) => v))).toBe(100);
  });

  it("treats a press without travel as a click: no change, no commit", () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    render(h(Harness, { onChange, onCommit }));
    const el = spin();
    drag(el, [50, 51, 52]);
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(onChange).not.toHaveBeenCalled();
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("Escape mid-drag reverts to the start value and never commits", () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    render(h(Harness, { initial: 20, sensitivity: 4, onChange, onCommit }));
    const el = spin();
    drag(el, [0, 10, 18, 26]);
    expect(onChange).toHaveBeenLastCalledWith(24);
    fireEvent.keyDown(el, { key: "Escape" });
    expect(onChange).toHaveBeenLastCalledWith(20);
    expect(el.getAttribute("aria-valuenow")).toBe("20");
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("losing pointer lock mid-drag cancels the gesture", () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    render(h(Harness, { initial: 20, sensitivity: 4, onChange, onCommit }));
    const el = spin();
    drag(el, [0, 10, 18]);
    let locked: Element | null = el;
    Object.defineProperty(document, "pointerLockElement", {
      configurable: true,
      get: () => locked,
    });
    document.dispatchEvent(new Event("pointerlockchange"));
    locked = null;
    document.dispatchEvent(new Event("pointerlockchange"));
    Reflect.deleteProperty(document, "pointerLockElement");
    expect(onChange).toHaveBeenLastCalledWith(20);
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(onCommit).not.toHaveBeenCalled();
  });
});

describe("NumberScrubber: typing", () => {
  it("Enter opens a text field, typing then Enter commits once", async () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { unit: "%", onChange, onCommit }));
    await user.tab();
    await user.keyboard("{Enter}");
    const input = screen.getByRole("textbox", { name: "Scale" });
    expect(input).toBe(document.activeElement);
    expect(document.querySelector(".cw-num")?.getAttribute("data-state")).toBe("editing");
    await user.keyboard("250{Enter}");
    expect(onChange).toHaveBeenCalledExactlyOnceWith(250);
    expect(onCommit).toHaveBeenCalledExactlyOnceWith(250);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(spin().getAttribute("aria-valuenow")).toBe("250");
    expect(document.activeElement).toBe(spin());
  });

  it("double-click starts editing", async () => {
    const user = userEvent.setup();
    render(h(Harness, {}));
    await user.dblClick(spin());
    expect(screen.getByRole("textbox", { name: "Scale" })).toBeTruthy();
  });

  it("Escape cancels without onChange or onCommit", async () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange, onCommit }));
    await user.tab();
    await user.keyboard("{Enter}999{Escape}");
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
    expect(onCommit).not.toHaveBeenCalled();
    expect(spin().getAttribute("aria-valuenow")).toBe("100");
  });

  it("clamps typed values, tolerates a typed unit, and rejects garbage", async () => {
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { max: 400, unit: "%", onCommit }));
    await user.tab();
    await user.keyboard("{Enter}999 %{Enter}");
    expect(onCommit).toHaveBeenLastCalledWith(400);
    await user.keyboard("{Enter}abc{Enter}");
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(spin().getAttribute("aria-valuenow")).toBe("400");
  });

  it("commits on blur exactly once, and not again after Enter", async () => {
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h("div", null, h(Harness, { onCommit }), h("button", { type: "button" }, "Other")));
    await user.tab();
    await user.keyboard("{Enter}7");
    await user.click(screen.getByRole("button", { name: "Other" }));
    expect(onCommit).toHaveBeenCalledExactlyOnceWith(7);
  });

  it("does not clear a mixed value on entering edit mode and needs explicit input", async () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(NumberScrubber, { label: "Scale", value: 100, mixed: true, onChange, onCommit }));
    await user.tab();
    await user.keyboard("{Enter}");
    expect((screen.getByRole("textbox") as HTMLInputElement).value).toBe("");
    await user.keyboard("{Enter}");
    expect(onChange).not.toHaveBeenCalled();
    await user.keyboard("{Enter}100{Enter}");
    expect(onCommit).toHaveBeenCalledExactlyOnceWith(100);
  });
});

describe("NumberScrubber: long decimals", () => {
  it("shows a rounded value but keeps the exact one", async () => {
    const onChange = vi.fn();
    render(h(Harness, { initial: 72.2582866787, unit: "%", onChange }));
    expect(spin().textContent).toContain("72.26");
    expect(spin().getAttribute("aria-valuenow")).toBe("72.2582866787");
    // Opening the editor and leaving it untouched must not overwrite the exact value.
    await userEvent.dblClick(spin());
    const input = screen.getByRole("textbox", { name: "Scale" }) as HTMLInputElement;
    expect(input.value).toBe("72.26");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).not.toHaveBeenCalled();
  });
});
