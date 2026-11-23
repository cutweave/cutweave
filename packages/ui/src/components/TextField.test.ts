// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TextField } from "./TextField.tsx";

afterEach(cleanup);

function Harness({ onChange, invalid }: { onChange: (v: string) => void; invalid?: boolean }) {
  const [value, setValue] = useState("");
  return h(TextField, {
    value,
    "aria-label": "Start",
    placeholder: "Mixed",
    numeric: true,
    invalid,
    onChange: (v: string) => {
      setValue(v);
      onChange(v);
    },
  });
}

describe("TextField", () => {
  it("is a named textbox that reports every edit", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(h(Harness, { onChange }));
    const field = screen.getByRole("textbox", { name: "Start" });
    await user.type(field, "00:01");
    expect(onChange).toHaveBeenLastCalledWith("00:01");
    expect((field as HTMLInputElement).value).toBe("00:01");
    expect(field.classList.contains("cw-field--numeric")).toBe(true);
  });

  it("flags an invalid value and shows the placeholder while empty", () => {
    render(h(Harness, { onChange: () => {}, invalid: true }));
    const field = screen.getByRole("textbox", { name: "Start" });
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(field.getAttribute("placeholder")).toBe("Mixed");
  });
});
