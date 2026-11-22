// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Switch } from "./Switch.tsx";

afterEach(cleanup);

function Harness({ onChange }: { onChange: (v: boolean) => void }) {
  const [checked, setChecked] = useState(false);
  return h(Switch, {
    checked,
    "aria-label": "Snap to clips",
    onCheckedChange: (v: boolean) => {
      setChecked(v);
      onChange(v);
    },
  });
}

describe("Switch", () => {
  it("is a role=switch reflecting aria-checked", async () => {
    const user = userEvent.setup();
    render(h(Harness, { onChange: () => {} }));
    const sw = screen.getByRole("switch", { name: "Snap to clips" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    await user.click(sw);
    expect(sw.getAttribute("aria-checked")).toBe("true");
  });

  it("toggles with Space from the keyboard", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.tab();
    await user.keyboard(" ");
    expect(onChange).toHaveBeenLastCalledWith(true);
    await user.keyboard(" ");
    expect(onChange).toHaveBeenLastCalledWith(false);
  });

  it("is named by a visible label", () => {
    render(
      h(
        "div",
        null,
        h("label", { htmlFor: "snap" }, "Snap to clips"),
        h(Switch, { id: "snap", checked: true, onCheckedChange: () => {} }),
      ),
    );
    expect(screen.getByRole("switch", { name: "Snap to clips" })).toBeTruthy();
  });

  it("does nothing when disabled and is skipped by Tab", async () => {
    const onCheckedChange = vi.fn();
    const user = userEvent.setup();
    render(h(Switch, { checked: false, "aria-label": "Locked", disabled: true, onCheckedChange }));
    await user.tab();
    await user.click(screen.getByRole("switch"));
    expect(onCheckedChange).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(document.body);
  });
});
