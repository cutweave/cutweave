// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Slider, type SliderKeyframe } from "./Slider.tsx";

afterEach(cleanup);

beforeAll(() => {
  // Radix Slider measures its thumb with ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

function Harness(props: {
  onChange?: (v: number) => void;
  onCommit?: (v: number) => void;
  keyframe?: SliderKeyframe;
  disabled?: boolean;
}) {
  const [value, setValue] = useState(50);
  return h(Slider, {
    label: "Opacity",
    value,
    min: 0,
    max: 100,
    format: (v: number) => `${v}%`,
    disabled: props.disabled,
    keyframe: props.keyframe,
    onChange: (v: number) => {
      setValue(v);
      props.onChange?.(v);
    },
    onCommit: props.onCommit,
  });
}

describe("Slider", () => {
  it("exposes role=slider with name, range, value and readout", () => {
    render(h(Harness, {}));
    const slider = screen.getByRole("slider", { name: "Opacity" });
    expect(slider.getAttribute("aria-valuenow")).toBe("50");
    expect(slider.getAttribute("aria-valuemin")).toBe("0");
    expect(slider.getAttribute("aria-valuemax")).toBe("100");
    expect(slider.getAttribute("aria-valuetext")).toBe("50%");
    expect(screen.getByText("50%")).toBeTruthy();
  });

  it("steps with arrows, x10 with Shift, and jumps with Home/End", async () => {
    const onChange = vi.fn();
    const onCommit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange, onCommit }));
    await user.tab();
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith(51);
    await user.keyboard("{ArrowLeft}{ArrowLeft}");
    expect(onChange).toHaveBeenLastCalledWith(49);
    await user.keyboard("{Shift>}{ArrowRight}{/Shift}");
    expect(onChange).toHaveBeenLastCalledWith(59);
    await user.keyboard("{Home}");
    expect(onChange).toHaveBeenLastCalledWith(0);
    await user.keyboard("{End}");
    expect(onChange).toHaveBeenLastCalledWith(100);
    expect(onCommit).toHaveBeenLastCalledWith(100);
    expect(screen.getByText("100%")).toBeTruthy();
  });

  it("is inert and aria-disabled when disabled", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    const { container } = render(h(Harness, { onChange, disabled: true }));
    expect(container.querySelector(".cw-slider")?.getAttribute("aria-disabled")).toBe("true");
    await user.tab();
    await user.keyboard("{ArrowRight}");
    expect(onChange).not.toHaveBeenCalled();
  });

  describe("built-in keyframe toggle", () => {
    it("renders the toggle named for the property and routes prev/toggle/next", async () => {
      const kf = { onToggle: vi.fn(), onPrev: vi.fn(), onNext: vi.fn() };
      const user = userEvent.setup();
      render(h(Harness, { keyframe: { state: "elsewhere", ...kf } }));
      const toggle = screen.getByRole("button", { name: "Add keyframe at playhead for Opacity" });
      expect(toggle.getAttribute("data-state")).toBe("elsewhere");
      await user.click(toggle);
      await user.click(screen.getByRole("button", { name: /Previous keyframe/ }));
      await user.click(screen.getByRole("button", { name: /Next keyframe/ }));
      expect(kf.onToggle).toHaveBeenCalledOnce();
      expect(kf.onPrev).toHaveBeenCalledOnce();
      expect(kf.onNext).toHaveBeenCalledOnce();
    });

    it("reflects the on-at-playhead state with aria-pressed", () => {
      render(h(Harness, { keyframe: { state: "on", onToggle: () => {} } }));
      const toggle = screen.getByRole("button", {
        name: "Remove keyframe at playhead for Opacity",
      });
      expect(toggle.getAttribute("aria-pressed")).toBe("true");
      expect(screen.queryByRole("button", { name: /Previous keyframe/ })).toBeNull();
    });

    it("disables the keyframe slot with the slider", () => {
      render(h(Harness, { disabled: true, keyframe: { state: "off", onToggle: () => {} } }));
      expect(
        (screen.getByRole("button", { name: "Keyframes unavailable" }) as HTMLButtonElement)
          .disabled,
      ).toBe(true);
    });
  });
});
