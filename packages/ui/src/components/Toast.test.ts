// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Toast, type ToastProps, ToastProvider } from "./Toast.tsx";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  // Radix Toast tracks swipes with pointer capture; jsdom has none.
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Props = Partial<ToastProps> & { onOpen?: (open: boolean) => void };

function Harness({ onOpen, ...props }: Props) {
  const [open, setOpen] = useState(true);
  return h(
    ToastProvider,
    null,
    h(Toast, {
      tone: "success",
      title: "Export complete",
      message: "Exported 00:00:58 to Shorts_final.mp4",
      ...props,
      open,
      onOpenChange: (v: boolean) => {
        setOpen(v);
        onOpen?.(v);
      },
    }),
  );
}

describe("Toast", () => {
  it("success renders with a tone icon, a title and a real message", () => {
    render(h(Harness, {}));
    const toast = document.querySelector(".cw-toast");
    expect(toast).toBeTruthy();
    expect(toast?.getAttribute("data-tone")).toBe("success");
    expect(toast?.querySelector(".cw-toast__icon")?.getAttribute("data-icon")).toBe("check");
    expect(screen.getAllByText("Export complete").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Exported 00:00:58 to Shorts_final.mp4").length).toBeGreaterThan(0);
  });

  it("errors carry an alert icon and no duplicate role=alert (Radix foreground announces them)", () => {
    render(h(Harness, { tone: "danger", title: "Generation failed" }));
    const toast = document.querySelector(".cw-toast");
    expect(toast?.getAttribute("data-tone")).toBe("danger");
    expect(toast?.getAttribute("role")).not.toBe("alert");
    expect(
      screen.queryAllByRole("alert").filter((el) => el.classList.contains("cw-toast")),
    ).toHaveLength(0);
    expect(toast?.querySelector("svg")?.getAttribute("data-icon")).toBe("circle-alert");
  });

  it("warnings render with a warning icon and no duplicate live role", () => {
    render(h(Harness, { tone: "warning", title: "Media missing" }));
    const toast = document.querySelector(".cw-toast");
    expect(toast?.getAttribute("role")).not.toBe("alert");
    expect(toast?.querySelector("svg")?.getAttribute("data-icon")).toBe("triangle-alert");
  });

  it("job variant shows a progressbar with values and a cancel that does not dismiss", async () => {
    const onCancel = vi.fn();
    const onOpen = vi.fn();
    const user = userEvent.setup();
    render(
      h(Harness, {
        tone: "job",
        title: "Exporting Shorts_final.mp4",
        message: "64% · about 20 s left",
        progress: 64,
        onCancel,
        cancelLabel: "Cancel export",
        onOpen,
      }),
    );
    const bar = screen.getByRole("progressbar", { name: "Exporting Shorts_final.mp4" });
    expect(bar.getAttribute("aria-valuenow")).toBe("64");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    await user.click(screen.getByRole("button", { name: "Cancel export" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("actions run their command and close the toast", async () => {
    const retry = vi.fn();
    const onOpen = vi.fn();
    const user = userEvent.setup();
    render(
      h(Harness, {
        tone: "danger",
        title: "Generation failed",
        actions: [
          { label: "Retry", onClick: retry },
          { label: "View log", onClick: () => {}, variant: "ghost" },
        ],
        onOpen,
      }),
    );
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(retry).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenCalledWith(false);
  });

  it("Dismiss button closes it", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onOpen }));
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onOpen).toHaveBeenCalledWith(false);
  });

  it("Escape dismisses the focused toast", async () => {
    const onOpen = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onOpen, tone: "warning", title: "Media missing" }));
    (document.querySelector(".cw-toast") as HTMLElement).focus();
    await user.keyboard("{Escape}");
    expect(onOpen).toHaveBeenCalledWith(false);
  });

  it("success auto-dismisses after its duration", () => {
    vi.useFakeTimers();
    const onOpen = vi.fn();
    render(h(Harness, { onOpen, duration: 1000 }));
    act(() => {
      vi.advanceTimersByTime(1100);
    });
    expect(onOpen).toHaveBeenCalledWith(false);
  });

  it("errors and jobs persist until dismissed", () => {
    vi.useFakeTimers();
    const onOpen = vi.fn();
    render(h(Harness, { onOpen, tone: "danger", title: "Failed" }));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(onOpen).not.toHaveBeenCalled();
  });
});
