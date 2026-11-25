// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h, Profiler, useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { compileSchema } from "./compile.ts";
import { NumberControl, TextControl } from "./controls.tsx";
import { FIXTURES, hex } from "./fixtures.ts";
import { SchemaForm, type SchemaFormProps } from "./SchemaForm.tsx";
import { type ChangeMeta, DEFAULT_STRINGS, type JsonSchema } from "./types.ts";
import { setPath } from "./values.ts";

afterEach(cleanup);

beforeAll(() => {
  // Radix Slider measures its thumb with ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

type Call = { path: string; value: unknown; meta: ChangeMeta };

/** A form bound to state, like an inspector bound to engine state. */
function Harness(
  props: Partial<SchemaFormProps> & { schema: JsonSchema; initial: unknown; calls?: Call[] },
) {
  const { initial, calls, ...rest } = props;
  const [value, setValue] = useState(initial);
  return h(SchemaForm, {
    "aria-label": "Properties",
    value,
    onChange: (path: string, next: unknown, meta: ChangeMeta) => {
      calls?.push({ path, value: next, meta });
      setValue((v: unknown) => setPath(v, path, next));
    },
    ...rest,
  });
}

function widgetMap(root: HTMLElement): Record<string, string> {
  const out: Record<string, string> = {};
  for (const el of root.querySelectorAll<HTMLElement>("[data-path][data-widget]")) {
    out[el.dataset.path ?? ""] = el.dataset.widget ?? "";
  }
  return out;
}

describe("fixtures", () => {
  it("has at least 25 of them", () => {
    expect(FIXTURES.length).toBeGreaterThanOrEqual(25);
  });

  it.each(FIXTURES.map((f) => [f.name, f] as const))(
    "renders %s with the expected widgets",
    (_name, fixture) => {
      const { container } = render(
        h(Harness, { schema: fixture.schema, initial: fixture.value, ...fixture.props }),
      );
      expect(widgetMap(container)).toEqual(fixture.widgets);
    },
  );

  it("records every fixture's widget map (snapshot)", () => {
    const all: Record<string, Record<string, string>> = {};
    for (const f of FIXTURES) {
      const { container, unmount } = render(
        h(Harness, { schema: f.schema, initial: f.value, ...f.props }),
      );
      all[f.name] = widgetMap(container);
      unmount();
    }
    expect(all).toMatchSnapshot();
  });

  it.each(FIXTURES.map((f) => [f.name, f] as const))(
    "%s has no axe violations",
    async (_name, fixture) => {
      const { container } = render(
        h(Harness, { schema: fixture.schema, initial: fixture.value, ...fixture.props }),
      );
      const results = await axe.run(container, {
        // jsdom has no layout or computed colours; contrast is covered by the Storybook axe suite.
        rules: { "color-contrast": { enabled: false } },
      });
      expect(
        results.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.html).join(" | ")}`),
      ).toEqual([]);
    },
  );
});

describe("controls report edits", () => {
  const schema: JsonSchema = {
    type: "object",
    properties: {
      reverse: { type: "boolean", default: false },
      fit: { type: "string", enum: ["fit", "fill", "stretch"], default: "fit" },
      blend: {
        type: "string",
        enum: ["normal", "multiply", "screen", "overlay", "add"],
        default: "normal",
      },
      name: { type: "string", default: "Untitled" },
      gain: {
        type: "number",
        minimum: -60,
        maximum: 12,
        default: 0,
        "x-ui": { unit: "dB", step: 0.5 },
      },
      opacity: {
        type: "number",
        default: 1,
        "x-ui": { widget: "slider", min: 0, max: 1, step: 0.01, unit: "%", scale: 100 },
      },
    },
  };
  const initial = {
    reverse: false,
    fit: "fit",
    blend: "normal",
    name: "Untitled",
    gain: 0,
    opacity: 1,
  };

  it("a toggle is one one-shot change", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial, calls }));
    await userEvent.click(screen.getByRole("switch", { name: "Reverse" }));
    expect(calls).toEqual([{ path: "reverse", value: true, meta: {} }]);
  });

  it("segmented and select pick by value, not by label", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial, calls }));
    await userEvent.click(screen.getByRole("radio", { name: "Fill" }));
    await userEvent.selectOptions(screen.getByLabelText("Blend"), "Screen");
    expect(calls.map((c) => [c.path, c.value])).toEqual([
      ["fit", "fill"],
      ["blend", "screen"],
    ]);
  });

  it("text commits on Enter and Escape restores the old text", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial, calls }));
    const field = screen.getByLabelText("Name");
    await userEvent.clear(field);
    await userEvent.type(field, "Intro{Enter}");
    expect(calls).toEqual([{ path: "name", value: "Intro", meta: {} }]);
    await userEvent.type(field, "X{Escape}");
    expect(calls).toHaveLength(1);
    expect((field as HTMLInputElement).value).toBe("Intro");
  });

  it("a number steps with the arrow keys using the hint's step and unit", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial, calls }));
    const gain = screen.getByRole("spinbutton", { name: "Gain" });
    expect(gain.getAttribute("aria-valuetext")).toBe("0 dB");
    gain.focus();
    await userEvent.keyboard("{ArrowUp}");
    expect(calls).toEqual([{ path: "gain", value: 0.5, meta: {} }]);
  });

  it("a scaled slider shows 0..100 % and reports 0..1", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial, calls }));
    const thumb = screen.getByRole("slider", { name: "Opacity" });
    expect(thumb.getAttribute("aria-valuetext")).toBe("100%");
    expect(thumb.getAttribute("aria-valuemax")).toBe("100");
    thumb.focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(calls).toEqual([{ path: "opacity", value: 0.99, meta: {} }]);
    expect(thumb.getAttribute("aria-valuetext")).toBe("99%");
  });
});

describe("gestures", () => {
  const schema: JsonSchema = {
    type: "object",
    properties: { x: { type: "number", default: 0, "x-ui": { step: 1 } } },
  };

  it("a scrub drag is start, at most 60 updates in a second, one end", () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance", "Date"] });
    try {
      const calls: Call[] = [];
      render(h(Harness, { schema, initial: { x: 0 }, calls }));
      const el = screen.getByRole("spinbutton", { name: "X" });
      fireEvent.pointerDown(el, { clientX: 0, button: 0, pointerId: 1 });
      // 400 moves (4 px = one step) in one second.
      for (let i = 1; i <= 400; i++) {
        fireEvent.pointerMove(el, { clientX: i * 4, pointerId: 1 });
        act(() => {
          vi.advanceTimersByTime(2.5);
        });
      }
      fireEvent.pointerUp(el, { pointerId: 1 });
      const by = (g: string) => calls.filter((c) => c.meta.gesture === g);
      expect(by("start")).toHaveLength(1);
      expect(by("end")).toHaveLength(1);
      expect(by("update").length).toBeLessThanOrEqual(60);
      expect(by("update").length).toBeGreaterThan(30);
      expect(calls[0]?.meta.gesture).toBe("start");
      expect(calls.at(-1)).toEqual({ path: "x", value: 399, meta: { gesture: "end" } });
    } finally {
      vi.useRealTimers();
    }
  });

  it("the control follows the pointer between throttled updates", () => {
    render(h(Harness, { schema, initial: { x: 0 } }));
    const el = screen.getByRole("spinbutton", { name: "X" });
    fireEvent.pointerDown(el, { clientX: 0, button: 0, pointerId: 1 });
    for (let i = 1; i <= 5; i++) fireEvent.pointerMove(el, { clientX: i * 4, pointerId: 1 });
    expect(el.getAttribute("aria-valuenow")).toBe("4");
    fireEvent.pointerUp(el, { pointerId: 1 });
    expect(screen.getByRole("spinbutton", { name: "X" }).getAttribute("aria-valuenow")).toBe("4");
  });

  it("Escape during a drag closes the gesture at the original value", () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial: { x: 7 }, calls }));
    const el = screen.getByRole("spinbutton", { name: "X" });
    fireEvent.pointerDown(el, { clientX: 0, button: 0, pointerId: 1 });
    for (let i = 1; i <= 6; i++) fireEvent.pointerMove(el, { clientX: i * 4, pointerId: 1 });
    fireEvent.keyDown(el, { key: "Escape" });
    expect(calls.at(-1)).toEqual({ path: "x", value: 7, meta: { gesture: "end" } });
    expect(calls.filter((c) => c.meta.gesture === "end")).toHaveLength(1);
  });

  it("a Radix slider drag streams through the same contract", () => {
    const calls: Call[] = [];
    const sliderSchema: JsonSchema = {
      type: "object",
      properties: {
        v: { type: "number", "x-ui": { widget: "slider", min: 0, max: 100, step: 1 } },
      },
    };
    const rect = {
      left: 0,
      top: 0,
      right: 200,
      bottom: 10,
      width: 200,
      height: 10,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    };
    const original = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = () => rect as DOMRect;
    Element.prototype.setPointerCapture = () => {};
    Element.prototype.hasPointerCapture = () => true;
    Element.prototype.releasePointerCapture = () => {};
    try {
      const { container } = render(h(Harness, { schema: sliderSchema, initial: { v: 0 }, calls }));
      const track = container.querySelector<HTMLElement>(".cw-slider__track");
      expect(track).not.toBeNull();
      const t = track as HTMLElement;
      fireEvent.pointerDown(t, { clientX: 10, button: 0, pointerId: 1 });
      for (let x = 20; x <= 180; x += 10) fireEvent.pointerMove(t, { clientX: x, pointerId: 1 });
      fireEvent.pointerUp(t, { clientX: 180, pointerId: 1 });
      expect(calls.filter((c) => c.meta.gesture === "end")).toHaveLength(1);
      expect(calls.filter((c) => c.meta.gesture === "start")).toHaveLength(1);
      expect(calls.at(-1)?.meta.gesture).toBe("end");
      expect(calls.at(-1)?.value).toBe(90);
    } finally {
      Element.prototype.getBoundingClientRect = original;
    }
  });
});

describe("reset, mixed, keyframes, visibility", () => {
  const schema: JsonSchema = {
    type: "object",
    properties: {
      scale: { type: "number", default: 1 },
      mode: { type: "string", enum: ["a", "b"], default: "a" },
      extra: { type: "number", "x-ui": { visibleWhen: "mode == 'b'" } },
    },
  };

  it("shows reset only while a value differs from its default", async () => {
    const calls: Call[] = [];
    render(h(Harness, { schema, initial: { scale: 1, mode: "a" }, calls }));
    expect(screen.queryByRole("button", { name: "Reset Scale" })).toBeNull();
    cleanup();
    render(h(Harness, { schema, initial: { scale: 2, mode: "a" }, calls }));
    await userEvent.click(screen.getByRole("button", { name: "Reset Scale" }));
    expect(calls).toEqual([{ path: "scale", value: 1, meta: {} }]);
    expect(screen.queryByRole("button", { name: "Reset Scale" })).toBeNull();
  });

  it("hands the reset to the host when it takes it", async () => {
    const onReset = vi.fn();
    const calls: Call[] = [];
    render(h(Harness, { schema, initial: { scale: 2, mode: "a" }, calls, onReset }));
    await userEvent.click(screen.getByRole("button", { name: "Reset Scale" }));
    expect(onReset).toHaveBeenCalledWith("scale");
    expect(calls).toEqual([]);
  });

  it("takes defaults from the host for properties the schema does not default", () => {
    render(h(Harness, { schema, initial: { scale: 1, mode: "a" }, defaults: { scale: 3 } }));
    expect(screen.getByRole("button", { name: "Reset Scale" })).toBeTruthy();
  });

  it("shows em dashes and 'Mixed' for a multi-selection", () => {
    render(
      h(Harness, {
        schema,
        initial: { scale: 1, mode: "a" },
        mixed: ["scale", "mode"],
      }),
    );
    expect(screen.getByRole("spinbutton", { name: "Scale" }).getAttribute("aria-valuetext")).toBe(
      "mixed",
    );
    expect(screen.getByRole("spinbutton", { name: "Scale" }).textContent).toContain("—");
    for (const radio of screen.getAllByRole("radio"))
      expect(radio.getAttribute("aria-checked")).toBe("false");
    // A mixed field can be reset to unify the selection.
    expect(screen.getByRole("button", { name: "Reset Scale" })).toBeTruthy();
  });

  it("toggles a dependent field with visibleWhen", async () => {
    render(h(Harness, { schema, initial: { scale: 1, mode: "a" } }));
    expect(screen.queryByRole("spinbutton", { name: "Extra" })).toBeNull();
    await userEvent.click(screen.getByRole("radio", { name: "B" }));
    expect(screen.getByRole("spinbutton", { name: "Extra" })).toBeTruthy();
  });

  it("draws the diamond only for fields the host wires, with the animated state", async () => {
    const toggle = vi.fn();
    const { container } = render(
      h(Harness, {
        schema: {
          type: "object",
          properties: {
            x: { type: "number", "x-ui": { keyframeable: true } },
            y: { type: "number" },
          },
        },
        initial: { x: 0, y: 0 },
        keyframes: { x: { state: "on", onToggle: toggle, onPrev: () => {}, onNext: () => {} } },
      }),
    );
    const toggleButton = screen.getByRole("button", { name: "Remove keyframe at playhead for X" });
    expect(toggleButton.getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "Previous keyframe (Alt+Left)" })).toBeTruthy();
    expect(container.querySelector('[data-path="x"]')?.getAttribute("data-animated")).toBe("on");
    expect(container.querySelector('[data-path="y"] .cw-kf')).toBeNull();
    await userEvent.click(toggleButton);
    expect(toggle).toHaveBeenCalledTimes(1);
  });

  it("without a keyframes prop no diamond shows", () => {
    const { container } = render(
      h(Harness, {
        schema: {
          type: "object",
          properties: { x: { type: "number", "x-ui": { keyframeable: true } } },
        },
        initial: { x: 0 },
      }),
    );
    expect(container.querySelector(".cw-kf")).toBeNull();
  });

  it("keeps a keyframeable field's slot, disabled, when the host has no handler for it", () => {
    render(
      h(Harness, {
        schema: {
          type: "object",
          properties: { x: { type: "number", "x-ui": { keyframeable: true } } },
        },
        initial: { x: 0 },
        keyframes: {},
      }),
    );
    expect(
      (screen.getByRole("button", { name: "Keyframes unavailable" }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });
});

describe("compound widgets", () => {
  it("vec2 edits report the whole value, and linking keeps the ratio", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: {
        scale: {
          type: "array",
          items: { type: "number" },
          minItems: 2,
          maxItems: 2,
          "x-ui": { widget: "vec2", step: 1 },
        },
      },
    };
    render(h(Harness, { schema, initial: { scale: [2, 1] }, calls }));
    const x = screen.getByRole("spinbutton", { name: "Scale X" });
    x.focus();
    await userEvent.keyboard("{ArrowUp}");
    expect(calls.at(-1)).toEqual({ path: "scale", value: [3, 1], meta: {} });
    await userEvent.click(screen.getByRole("button", { name: "Link X and Y of Scale" }));
    screen.getByRole("spinbutton", { name: "Scale X" }).focus();
    await userEvent.keyboard("{ArrowUp}");
    // 3 -> 4 scales y by 4/3.
    const last = calls.at(-1)?.value as number[];
    expect(last[0]).toBe(4);
    expect(last[1]).toBeCloseTo(4 / 3, 5);
  });

  it("vec2 object keeps the other keys of the value", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: {
        pos: {
          type: "object",
          properties: { x: { type: "number" }, y: { type: "number" } },
          "x-ui": { step: 1 },
        },
      },
    };
    render(h(Harness, { schema, initial: { pos: { x: 1, y: 2 } }, calls }));
    screen.getByRole("spinbutton", { name: "Pos Y" }).focus();
    await userEvent.keyboard("{ArrowUp}");
    expect(calls.at(-1)).toEqual({ path: "pos", value: { x: 1, y: 3 }, meta: {} });
  });

  it("array add and remove report the whole list and respect the limits", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: { tags: { type: "array", items: { type: "string" }, maxItems: 2 } },
    };
    render(h(Harness, { schema, initial: { tags: ["a"] }, calls }));
    await userEvent.click(screen.getByRole("button", { name: "Add Tags" }));
    expect(calls.at(-1)).toEqual({ path: "tags", value: ["a", ""], meta: {} });
    expect((screen.getByRole("button", { name: "Add Tags" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    await userEvent.click(screen.getByRole("button", { name: "Remove Tags 1" }));
    expect(calls.at(-1)).toEqual({ path: "tags", value: [""], meta: {} });
  });

  it("array elements edit by indexed path", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: { tags: { type: "array", items: { type: "string" } } },
    };
    render(h(Harness, { schema, initial: { tags: ["a", "b"] }, calls }));
    const second = screen.getByLabelText("Tags 2");
    await userEvent.clear(second);
    await userEvent.type(second, "z{Enter}");
    expect(calls.at(-1)).toEqual({ path: "tags.1", value: "z", meta: {} });
  });

  it("a tuple reports the whole array", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: {
        lift: {
          type: "array",
          items: { type: "number" },
          minItems: 3,
          maxItems: 3,
          "x-ui": { step: 1 },
        },
      },
    };
    render(h(Harness, { schema, initial: { lift: [0, 1, 2] }, calls }));
    screen.getByRole("spinbutton", { name: "Lift 2" }).focus();
    await userEvent.keyboard("{ArrowUp}");
    expect(calls.at(-1)).toEqual({ path: "lift", value: [0, 2, 2], meta: {} });
  });

  it("color: hex text commits a valid color and keeps the alpha format", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: { fill: { type: "string", "x-ui": { widget: "color" } } },
    };
    render(h(Harness, { schema, initial: { fill: hex("336699") }, calls }));
    const field = screen.getByLabelText("Fill hex");
    await userEvent.clear(field);
    await userEvent.type(field, "ff0000{Enter}");
    expect(calls.at(-1)).toEqual({ path: "fill", value: hex("ff0000"), meta: {} });
    screen.getByRole("spinbutton", { name: "Fill alpha" }).focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(calls.at(-1)).toEqual({ path: "fill", value: hex("ff0000fc"), meta: {} });
  });

  it("angle: the dial is a slider with degree text and arrow keys", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: {
        rotation: { type: "number", "x-ui": { widget: "angle", min: -180, max: 180 } },
      },
    };
    render(h(Harness, { schema, initial: { rotation: 90 }, calls }));
    const dial = screen.getByRole("slider", { name: "Rotation dial" });
    expect(dial.getAttribute("aria-valuetext")).toBe("90°");
    dial.focus();
    await userEvent.keyboard("{ArrowRight}");
    expect(calls.at(-1)).toEqual({ path: "rotation", value: 91, meta: {} });
  });

  it("curve: a host renderer replaces the default select", () => {
    const schema: JsonSchema = {
      type: "object",
      properties: {
        easing: { type: "string", enum: ["linear", "bounce"], "x-ui": { widget: "curve" } },
      },
    };
    render(
      h(Harness, {
        schema,
        initial: { easing: "linear" },
        renderers: {
          curve: (p: { label: string }) => h("div", { "data-testid": "curve" }, p.label),
        },
      }),
    );
    expect(screen.getByTestId("curve").textContent).toBe("Easing");
  });

  it("asset: offers the bin, filtered by kind, and None when optional", async () => {
    const calls: Call[] = [];
    const schema: JsonSchema = {
      type: "object",
      properties: { lut: { type: "string", "x-ui": { widget: "asset", kind: "image" } } },
    };
    render(
      h(Harness, {
        schema,
        initial: {},
        calls,
        assets: [
          { id: "m1", label: "Warm.cube", kind: "image" },
          { id: "m2", label: "Intro.mp4", kind: "video" },
        ],
      }),
    );
    const select = screen.getByLabelText("Lut");
    const labels = within(select)
      .getAllByRole("option")
      .map((o) => o.textContent);
    expect(labels).toEqual(["None", "Warm.cube"]);
    await userEvent.selectOptions(select, "Warm.cube");
    expect(calls.at(-1)).toEqual({ path: "lut", value: "m1", meta: {} });
  });

  it("groups into sections, with Advanced collapsed", async () => {
    const schema: JsonSchema = {
      type: "object",
      properties: {
        a: { type: "number", "x-ui": { group: "Look" } },
        b: { type: "number", "x-ui": { advanced: true } },
      },
    };
    render(h(Harness, { schema, initial: { a: 1, b: 2 } }));
    const look = screen.getByRole("button", { name: "Look" });
    expect(look.getAttribute("aria-expanded")).toBe("true");
    const adv = screen.getByRole("button", { name: "Advanced" });
    expect(adv.getAttribute("aria-expanded")).toBe("false");
    await userEvent.click(adv);
    expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  it("lets a locale replace every string", () => {
    render(
      h(Harness, {
        schema: { type: "object", properties: { x: { type: "number", default: 1 } } },
        initial: { x: 2 },
        strings: { reset: (l: string) => `Zurücksetzen ${l}` },
      }),
    );
    expect(screen.getByRole("button", { name: "Zurücksetzen X" })).toBeTruthy();
  });
});

describe("performance", () => {
  // jsdom with React's development build: every element creation captures a stack (React 19), so
  // absolute mount times here are several times the production figure. The row chrome (labels,
  // reset, keyframes, layout) is what this package adds, so it is measured as the mount cost over
  // the same 40 bare controls; the update path, the one that runs at 60 Hz, is absolute.
  const properties: Record<string, JsonSchema> = {};
  const initial: Record<string, unknown> = {};
  for (let i = 0; i < 40; i++) {
    properties[`f${i}`] =
      i % 2 === 0 ? { type: "number", default: 0 } : { type: "string", default: "" };
    initial[`f${i}`] = i % 2 === 0 ? 0 : "";
  }
  const schema: JsonSchema = { type: "object", properties };

  /** One mount's render time (the profiler's `actualDuration`). */
  function mountOnce(make: () => ReturnType<typeof h>): number {
    let duration = Number.POSITIVE_INFINITY;
    const { unmount } = render(
      h(
        Profiler,
        {
          id: "bench",
          onRender: (_id, phase, actual) => {
            if (phase === "mount") duration = actual;
          },
        },
        make(),
      ),
    );
    unmount();
    return duration;
  }

  it("the row chrome of 40 fields costs under 8 ms over the bare controls", {
    retry: 2,
    timeout: 60_000,
  }, () => {
    const nodes = compileSchema(schema);
    const noop = () => {};
    const bare = () =>
      h(
        "div",
        null,
        nodes.map((node) =>
          h(node.widget === "number" ? NumberControl : TextControl, {
            key: node.path,
            node,
            label: node.label,
            id: node.path,
            labelId: node.path,
            value: initial[node.key],
            mixed: false,
            disabled: false,
            strings: DEFAULT_STRINGS,
            live: noop,
            commit: noop,
            cancel: noop,
          }),
        ),
      );
    const form = () => h(SchemaForm, { schema, value: initial, onChange: noop });
    // Best of N, alternating so both see the same load. N grows (up to 300) until a
    // quiet moment shows the budget is met: alone it takes about 2 ms, but the full suite runs on
    // several workers and the minimum needs a window where the machine is not busy.
    let formBest = Number.POSITIVE_INFINITY;
    let bareBest = Number.POSITIVE_INFINITY;
    for (let run = 0; run < 300; run++) {
      formBest = Math.min(formBest, mountOnce(form));
      bareBest = Math.min(bareBest, mountOnce(bare));
      if (run >= 20 && formBest - bareBest < 8) break;
    }
    const overhead = formBest - bareBest;
    expect(overhead).toBeLessThan(8);
  });

  it("re-rendering 40 fields after one edit takes under 8 ms", { retry: 2 }, () => {
    const durations: number[] = [];
    const onRender = (_id: string, phase: string, actual: number) => {
      if (phase === "update") durations.push(actual);
    };
    const make = (value: unknown) =>
      h(Profiler, { id: "bench", onRender }, h(SchemaForm, { schema, value, onChange: () => {} }));
    const { rerender } = render(make(initial));
    for (let i = 1; i <= 15; i++) rerender(make({ ...initial, f0: i }));
    expect(Math.min(...durations)).toBeLessThan(8);
  });

  it("only the edited row re-renders", () => {
    const seen: string[] = [];
    const Probe = (p: { id: string }) => {
      seen.push(p.id);
      return null;
    };
    // The probe renders through a host renderer for `curve` fields.
    const probeSchema: JsonSchema = {
      type: "object",
      properties: {
        a: { type: "string", enum: ["x", "y"], "x-ui": { widget: "curve" } },
        b: { type: "string", enum: ["x", "y"], "x-ui": { widget: "curve" } },
      },
    };
    const renderers = { curve: (p: { label: string }) => h(Probe, { id: p.label }) };
    const props = { schema: probeSchema, onChange: () => {}, renderers };
    const { rerender } = render(h(SchemaForm, { ...props, value: { a: "x", b: "x" } }));
    seen.length = 0;
    rerender(h(SchemaForm, { ...props, value: { a: "y", b: "x" } }));
    expect(seen).toEqual(["A"]);
  });
});
