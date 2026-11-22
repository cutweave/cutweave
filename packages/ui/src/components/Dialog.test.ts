// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button } from "./Button.tsx";
import { Dialog, DialogNote } from "./Dialog.tsx";

afterEach(cleanup);

interface HarnessProps {
  onSubmit?: () => void;
  dirty?: boolean;
  onDismissAttempt?: () => void;
  description?: string;
}

function Harness({ onSubmit, dirty, onDismissAttempt, description }: HarnessProps) {
  const [open, setOpen] = useState(false);
  return h(
    "div",
    null,
    h("button", { type: "button", id: "outside" }, "Outside"),
    h(Dialog, {
      open,
      onOpenChange: setOpen,
      title: "Export video",
      description,
      onSubmit,
      dirty,
      onDismissAttempt,
      trigger: h(Button, null, "Export…"),
      footer: [
        h(
          Button,
          { key: "c", variant: "ghost", size: "lg", onClick: () => setOpen(false) },
          "Cancel",
        ),
        h(Button, { key: "e", variant: "primary", size: "lg" }, "Export"),
      ],
      children: [
        h("label", { key: "l", htmlFor: "preset" }, "Preset"),
        h("input", { key: "i", id: "preset" }),
        h("textarea", { key: "t", "aria-label": "Notes" }),
        h(DialogNote, { key: "n", children: "Renders locally. Free (local)." }),
      ],
    }),
  );
}

async function openDialog() {
  const user = userEvent.setup();
  render(h(Harness, {}));
  return { user, ...(await open(user)) };
}

async function open(user: ReturnType<typeof userEvent.setup>) {
  const opener = screen.getByRole("button", { name: "Export…" });
  await user.click(opener);
  const dialog = await screen.findByRole("dialog");
  return { opener, dialog };
}

describe("Dialog", () => {
  it("is a modal dialog named by its title", async () => {
    const { dialog } = await openDialog();
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    const labelledBy = dialog.getAttribute("aria-labelledby");
    expect(document.getElementById(labelledBy as string)?.textContent).toBe("Export video");
    expect(screen.getByRole("dialog", { name: "Export video" })).toBe(dialog);
    expect(screen.getByRole("heading", { name: "Export video" })).toBeTruthy();
  });

  it("wires an optional description", async () => {
    const user = userEvent.setup();
    render(h(Harness, { description: "Choose a preset and range." }));
    const { dialog } = await open(user);
    const describedBy = dialog.getAttribute("aria-describedby");
    expect(document.getElementById(describedBy as string)?.textContent).toBe(
      "Choose a preset and range.",
    );
  });

  it("moves focus to the first field on open", async () => {
    await openDialog();
    expect(document.activeElement).toBe(screen.getByLabelText("Preset"));
  });

  it("traps focus: Tab and Shift+Tab cycle inside the dialog", async () => {
    const { user, dialog } = await openDialog();
    for (let i = 0; i < 8; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    for (let i = 0; i < 8; i++) {
      await user.tab({ shift: true });
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    expect(document.activeElement).not.toBe(document.getElementById("outside"));
  });

  it("makes the page behind inert to assistive tech while open", async () => {
    await openDialog();
    expect(screen.queryByRole("button", { name: "Outside" })).toBeNull();
  });

  it("Escape closes and returns focus to the opener", async () => {
    const { user, opener } = await openDialog();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(opener);
  });

  it("Cancel closes and returns focus to the opener", async () => {
    const { user, opener } = await openDialog();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(opener);
  });

  it("Enter in a single-line field runs onSubmit; not in a textarea or on a button", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onSubmit }));
    await open(user);
    await user.keyboard("{Enter}");
    expect(onSubmit).toHaveBeenCalledTimes(1);
    await user.click(screen.getByLabelText("Notes"));
    await user.keyboard("{Enter}");
    expect(onSubmit).toHaveBeenCalledTimes(1);
    screen.getByRole("button", { name: "Cancel" }).focus();
    await user.keyboard("{Enter}");
    expect(onSubmit).toHaveBeenCalledTimes(1);
  });

  it("with unsaved changes, Escape does not close and reports the attempt", async () => {
    const onDismissAttempt = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { dirty: true, onDismissAttempt }));
    await open(user);
    await user.keyboard("{Escape}");
    expect(onDismissAttempt).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  it("renders the cost note with an icon and words", async () => {
    await openDialog();
    const note = screen.getByText("Renders locally. Free (local).").closest(".cw-note");
    expect(note?.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("restores focus to the previously focused element when driven without a trigger", async () => {
    function Controlled() {
      const [isOpen, setIsOpen] = useState(false);
      return h(
        "div",
        null,
        h("button", { type: "button", onClick: () => setIsOpen(true) }, "Open it"),
        h(Dialog, {
          open: isOpen,
          onOpenChange: setIsOpen,
          title: "Confirm",
          children: h("input", { "aria-label": "Name" }),
        }),
      );
    }
    const user = userEvent.setup();
    render(h(Controlled));
    const opener = screen.getByRole("button", { name: "Open it" });
    await user.click(opener);
    await screen.findByRole("dialog");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(opener);
  });
});
