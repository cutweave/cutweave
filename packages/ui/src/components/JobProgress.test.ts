// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { JobList, JobProgress } from "./JobProgress.tsx";

afterEach(cleanup);

describe("JobProgress", () => {
  it("running: labelled progressbar with values, percent text, ETA and a named cancel", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(
      h(
        JobList,
        { "aria-label": "Background jobs" },
        h(JobProgress, {
          name: "Exporting Shorts_final.mp4",
          state: "running",
          progress: 64,
          eta: "20 s left",
          onCancel,
        }),
      ),
    );
    expect(screen.getByRole("list", { name: "Background jobs" })).toBeTruthy();
    expect(screen.getAllByRole("listitem")).toHaveLength(1);
    const bar = screen.getByRole("progressbar", { name: "Exporting Shorts_final.mp4" });
    expect(bar.getAttribute("aria-valuenow")).toBe("64");
    expect(bar.getAttribute("aria-valuemin")).toBe("0");
    expect(bar.getAttribute("aria-valuemax")).toBe("100");
    expect((bar as HTMLElement).style.getPropertyValue("--value")).toBe("64%");
    expect(screen.getByText("64%")).toBeTruthy();
    expect(screen.getByText("20 s left")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Cancel Exporting Shorts_final.mp4" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("clamps out-of-range progress", () => {
    render(h(JobProgress, { name: "Job", state: "running", progress: 140 }));
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("100");
    expect(screen.getByText("100%")).toBeTruthy();
  });

  it("queued: shows the Queued chip with icon and word instead of a bar", () => {
    render(h(JobProgress, { name: "Transcribing interview_A.mov", state: "queued", progress: 0 }));
    expect(screen.getByText("Queued")).toBeTruthy();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("failed: announces the error in words, keeps the bar and offers Retry instead of Cancel", async () => {
    const onRetry = vi.fn();
    const user = userEvent.setup();
    render(
      h(JobProgress, {
        name: "Rendering proxy for drone_03.mov",
        state: "failed",
        progress: 42,
        error: "Failed: source file not found",
        onRetry,
      }),
    );
    expect(screen.getByRole("alert").textContent).toBe("Failed: source file not found");
    expect(screen.getByRole("progressbar").getAttribute("aria-valuenow")).toBe("42");
    expect(screen.queryByRole("button", { name: /Cancel/ })).toBeNull();
    await user.click(
      screen.getByRole("button", { name: "Retry Rendering proxy for drone_03.mov" }),
    );
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("cancel is reachable and activates from the keyboard", async () => {
    const onCancel = vi.fn();
    const user = userEvent.setup();
    render(h(JobProgress, { name: "Job", state: "running", progress: 10, onCancel }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel Job" }));
    await user.keyboard("{Enter}");
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
