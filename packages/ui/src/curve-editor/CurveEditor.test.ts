// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2C.05: the curve editor. Bezier handles move with the pointer and the arrow keys (one
// commit per gesture), presets and kinds are picked, spring numbers are edited, the preview is
// what `sample` returns, and axe finds nothing.
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CurveEditor, type CurveEditorProps, type CurveValue } from "./CurveEditor.tsx";
import { SAMPLE_PRESETS, sampleCurve } from "./fixtures.ts";
import { clampHandle, curvePath, PLOT, pointFromClient, toX, toY, Y_MAX } from "./geometry.ts";

afterEach(cleanup);

const EASE: CurveValue = { kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 };

function mount(props: Partial<CurveEditorProps> = {}) {
  const onCommit = vi.fn<(v: CurveValue) => void>();
  const onChange = vi.fn<(v: CurveValue) => void>();
  const sample = vi.fn(sampleCurve);
  const view = render(
    h(CurveEditor, {
      value: EASE,
      onCommit,
      onChange,
      presets: SAMPLE_PRESETS,
      sample,
      ...props,
    }),
  );
  return { onCommit, onChange, sample, ...view };
}

describe("geometry", () => {
  it("maps the unit square into the graph and back", () => {
    const box = { left: 10, top: 20, width: PLOT.width * 2, height: PLOT.height * 2 };
    const p = pointFromClient(box, 10 + toX(0.25) * 2, 20 + toY(0.5) * 2);
    expect(p.x).toBeCloseTo(0.25, 3);
    expect(p.y).toBeCloseTo(0.5, 3);
  });

  it("keeps a handle's time in 0..1 and its progress in view", () => {
    expect(clampHandle(-1, 5)).toEqual({ x: 0, y: Y_MAX });
    expect(clampHandle(2, 0.5)).toEqual({ x: 1, y: 0.5 });
  });

  it("draws one path point per sample", () => {
    const d = curvePath([0, 0.5, 1]);
    expect(d.match(/[ML]/g)).toHaveLength(3);
    expect(d.startsWith(`M${toX(0).toFixed(2)} ${toY(0).toFixed(2)}`)).toBe(true);
  });
});

describe("CurveEditor", () => {
  it("draws the preview from the samples it is given", () => {
    const { sample } = mount();
    expect(sample).toHaveBeenCalledWith(EASE, 64);
    const path = screen.getByTestId("curve-preview").getAttribute("d") ?? "";
    expect(path).toBe(curvePath(sampleCurve(EASE, 64)));
  });

  it("names the current preset, and shows Custom for other numbers", () => {
    const { rerender, onCommit } = mount();
    const select = screen.getByLabelText("Preset") as HTMLSelectElement;
    expect(select.value).toBe("ease_in_out");
    rerender(
      h(CurveEditor, {
        value: { kind: "bezier", x1: 0.1, y1: 0.2, x2: 0.3, y2: 0.4 },
        onCommit,
        presets: SAMPLE_PRESETS,
        sample: sampleCurve,
      }),
    );
    expect((screen.getByLabelText("Preset") as HTMLSelectElement).value).toBe("");
    expect(screen.getByRole("option", { name: "Custom" })).toBeTruthy();
  });

  it("picking a preset commits its interpolation once", async () => {
    const user = userEvent.setup();
    const { onCommit } = mount();
    await user.selectOptions(screen.getByLabelText("Preset"), "back_out");
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "bezier", y1: 1.56, preset: "back_out" }),
    );
  });

  it("switching the kind commits that kind's default", async () => {
    const user = userEvent.setup();
    const { onCommit } = mount();
    await user.click(screen.getByRole("radio", { name: "Hold" }));
    expect(onCommit).toHaveBeenLastCalledWith({ kind: "hold" });
    await user.click(screen.getByRole("radio", { name: "Spring" }));
    expect(onCommit).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: "spring", stiffness: 170, damping: 26 }),
    );
  });

  it("arrow keys nudge a handle by 0.01, Shift by 0.1, one commit per key", async () => {
    const user = userEvent.setup();
    const { onCommit } = mount();
    const start = screen.getByRole("slider", { name: "Start handle" });
    start.focus();
    await user.keyboard("{ArrowRight}");
    expect(onCommit).toHaveBeenLastCalledWith({ kind: "bezier", x1: 0.43, y1: 0, x2: 0.58, y2: 1 });
    await user.keyboard("{ArrowUp}");
    expect(onCommit).toHaveBeenLastCalledWith({
      kind: "bezier",
      x1: 0.42,
      y1: 0.01,
      x2: 0.58,
      y2: 1,
    });
    await user.keyboard("{Shift>}{ArrowLeft}{/Shift}");
    expect(onCommit).toHaveBeenLastCalledWith({ kind: "bezier", x1: 0.32, y1: 0, x2: 0.58, y2: 1 });
    expect(onCommit).toHaveBeenCalledTimes(3);
    const end = screen.getByRole("slider", { name: "End handle" });
    end.focus();
    await user.keyboard("{ArrowDown}");
    expect(onCommit).toHaveBeenLastCalledWith({
      kind: "bezier",
      x1: 0.42,
      y1: 0,
      x2: 0.58,
      y2: 0.99,
    });
  });

  it("a handle is reachable with Tab and says where it is", async () => {
    const user = userEvent.setup();
    mount();
    const start = screen.getByRole("slider", { name: "Start handle" });
    for (let i = 0; i < 12 && document.activeElement !== start; i++) await user.tab();
    expect(document.activeElement).toBe(start);
    expect(start.getAttribute("aria-valuetext")).toBe("time 0.42, value 0.00");
  });

  it("dragging a handle previews live and commits once on release", () => {
    const { onCommit, onChange, container } = mount();
    const svg = container.querySelector("svg") as SVGSVGElement;
    svg.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: PLOT.width, height: PLOT.height }) as DOMRect;
    const end = screen.getByRole("slider", { name: "End handle" });
    fireEvent.pointerDown(end, { pointerId: 1, button: 0, clientX: toX(0.58), clientY: toY(1) });
    fireEvent.pointerMove(end, { pointerId: 1, clientX: toX(0.7), clientY: toY(0.8) });
    fireEvent.pointerMove(end, { pointerId: 1, clientX: toX(0.8), clientY: toY(0.6) });
    expect(onChange).toHaveBeenCalledTimes(2);
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(end, { pointerId: 1, clientX: toX(0.8), clientY: toY(0.6) });
    expect(onCommit).toHaveBeenCalledTimes(1);
    const committed = onCommit.mock.calls[0]?.[0] as Extract<CurveValue, { kind: "bezier" }>;
    expect(committed.x2).toBeCloseTo(0.8, 2);
    expect(committed.y2).toBeCloseTo(0.6, 2);
    expect(committed.x1).toBe(0.42);
    expect("preset" in committed).toBe(false);
  });

  it("a click on a handle without moving commits nothing", () => {
    const { onCommit } = mount();
    const start = screen.getByRole("slider", { name: "Start handle" });
    fireEvent.pointerDown(start, { pointerId: 1, button: 0 });
    fireEvent.pointerUp(start, { pointerId: 1 });
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("shows the spring's numbers and no handles for a spring", async () => {
    const user = userEvent.setup();
    const { onCommit } = mount({
      value: { kind: "spring", stiffness: 170, damping: 26, mass: 1, bounce: false },
    });
    expect(screen.queryByRole("slider", { name: "Start handle" })).toBeNull();
    expect(screen.getByText("Stiffness")).toBeTruthy();
    await user.click(screen.getByRole("switch", { name: "Bounce" }));
    expect(onCommit).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "spring", bounce: true, stiffness: 170 }),
    );
  });

  it("offers only the kinds the property accepts", () => {
    mount({ value: { kind: "hold" }, kinds: ["hold"] });
    expect((screen.getByRole("radio", { name: "Linear" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(screen.queryByRole("option", { name: "Ease in" })).toBeNull();
    expect(screen.getByRole("option", { name: "Hold" })).toBeTruthy();
  });

  it("disabled: nothing commits", async () => {
    const user = userEvent.setup();
    const { onCommit } = mount({ disabled: true });
    const start = screen.getByRole("slider", { name: "Start handle" });
    fireEvent.keyDown(start, { key: "ArrowRight" });
    await user.click(screen.getByRole("radio", { name: "Hold" }));
    expect(onCommit).not.toHaveBeenCalled();
  });

  it.each([
    ["bezier", EASE],
    ["spring", { kind: "spring", stiffness: 170, damping: 26, mass: 1, bounce: false }],
    ["hold", { kind: "hold" }],
  ] as const)("passes axe (%s)", async (_name, value) => {
    const { container } = mount({ value: value as CurveValue });
    const results = await axe.run(container, {
      // jsdom has no layout: colour contrast is checked by the Storybook axe run.
      rules: { "color-contrast": { enabled: false } },
    });
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([]);
  });
});
