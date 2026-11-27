// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TextArea } from "./TextArea.tsx";

afterEach(cleanup);

function Harness(props: { onChange?: (v: string) => void; invalid?: boolean }) {
  const [value, setValue] = useState("");
  return h(TextArea, {
    value,
    "aria-label": "Note",
    invalid: props.invalid,
    onChange: (v: string) => {
      setValue(v);
      props.onChange?.(v);
    },
  });
}

describe("TextArea", () => {
  it("is a named multi-line text box that reports every keystroke, line breaks too", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    const field = screen.getByRole("textbox", { name: "Note" }) as HTMLTextAreaElement;
    expect(field.tagName).toBe("TEXTAREA");
    await user.type(field, "one{Enter}two");
    expect(field.value).toBe("one\ntwo");
    expect(onChange).toHaveBeenLastCalledWith("one\ntwo");
  });

  it("says when its text is not valid", () => {
    render(h(Harness, { invalid: true }));
    expect(screen.getByRole("textbox", { name: "Note" }).getAttribute("aria-invalid")).toBe("true");
  });
});
