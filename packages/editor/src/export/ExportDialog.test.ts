// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-1M.08: the export dialog and the top bar's chip, over the real stores and a recording
// engine. `apps/desktop/e2e/export-dialog.spec.ts` runs the same flow in the app.
import type { JobInfo, Project } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { initialSessionState, sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { ExportStatusChip } from "./ExportChip.tsx";
import { ExportDialog } from "./ExportDialog.tsx";
import { exportStore } from "./exportStore.ts";
import { exportController, resetExportController } from "./useExport.ts";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView ??= () => {};
});

const JOB = "job_1" as JobInfo["id"];

const project = {
  meta: { name: "Film" },
  settings: {},
  media: {},
  sequences: {
    seq_1: {
      id: "seq_1",
      name: "Main",
      format: { width: 3840, height: 2160, frame_rate: { num: 30000, den: 1001 } },
      tracks: { trk_1: { id: "trk_1", items: { itm_1: { id: "itm_1" } } } },
      track_order: ["trk_1"],
      custom: {},
    },
  },
  custom: {},
} as unknown as Project;

const running = (fraction: number, message: string): JobInfo => ({
  id: JOB,
  kind: "export",
  label: "export",
  media_id: null,
  state: "running",
  progress: { fraction, message },
  cancel_requested: false,
  error: null,
});

let call: ReturnType<typeof vi.fn>;

const mount = () =>
  render(
    h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(ExportDialog), h(ExportStatusChip))),
  );

beforeEach(() => {
  resetExportController();
  call = vi.fn(async (name: string) => {
    if (name === "media.capabilities") return { hw_encode: [{ implementation: "h264_amf" }] };
    if (name === "export.start") return { job_id: JOB, path: "C:\\p\\Film.mp4" };
    if (name === "job.cancel") return { ...running(0.2, ""), cancel_requested: true };
    throw new Error(`unexpected ${name}`);
  });
  engineStore.setState({ ...initialEngineState, rpc: { call } as never, connection: "ready" });
  sessionStore.setState({
    ...initialSessionState,
    known: true,
    path: "C:\\p\\Film\\Film.cutweave",
    name: "Film",
  });
  docStore.setState({ project } as never);
  jobStore.setState({ jobs: {} });
  exportStore.setState({
    dialogOpen: false,
    active: null,
    cancelling: false,
    probe: { status: "idle" },
  });
  toastStore.getState().clear();
  exportController().attach();
});

afterEach(() => {
  cleanup();
  resetExportController();
});

describe("ExportDialog", () => {
  it("starts from the sequence: the preset's 1080p size, its rate, the project's folder", async () => {
    mount();
    act(() => exportController().open());
    const dialog = await screen.findByRole("dialog", { name: "Export video" });
    expect(within(dialog).getByLabelText("Save to")).toHaveProperty(
      "value",
      "C:\\p\\Film\\Film.mp4",
    );
    expect(within(dialog).getByLabelText("Width")).toHaveProperty("value", "1920");
    expect(within(dialog).getByLabelText("Height")).toHaveProperty("value", "1080");
    expect(within(dialog).getByLabelText("Frame rate")).toHaveProperty("value", "29.97");
    expect(await within(dialog).findByText("Found AMD AMF. Faster than software.")).toBeTruthy();
  });

  it("sends the preset by default and a custom export once a value changes", async () => {
    const user = userEvent.setup();
    mount();
    act(() => exportController().open());
    const dialog = await screen.findByRole("dialog", { name: "Export video" });
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("export.start", expect.anything()));
    expect(call).toHaveBeenLastCalledWith("export.start", {
      sequence: "seq_1",
      path: "C:\\p\\Film\\Film.mp4",
      preset: "h264-mp4-1080p",
      hw: "auto",
      overwrite: false,
    });
    expect(exportStore.getState().active?.jobId).toBe(JOB);

    // Running: the dialog now shows the progress.
    act(() =>
      jobStore.getState().applyProgress(running(0.4, "frame 12 of 30, 20.5 fps, about 0:03 left")),
    );
    const progress = await screen.findByRole("dialog", { name: "Exporting Film.mp4" });
    expect(within(progress).getByRole("progressbar").getAttribute("aria-valuenow")).toBe("40");
    expect(within(progress).getByText("20.5 fps · about 0:03 left")).toBeTruthy();
    expect(screen.getByTestId("export-chip").textContent).toBe("Exporting 40%");

    // Hide: the export goes on and the chip stays; it brings the dialog back.
    await user.click(within(progress).getByRole("button", { name: "Hide" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(exportStore.getState().active).not.toBeNull();
    await user.click(screen.getByTestId("export-chip"));
    expect(await screen.findByRole("dialog", { name: "Exporting Film.mp4" })).toBeTruthy();
  });

  it("turns the hardware switch into hw off and a changed size into a custom preset", async () => {
    const user = userEvent.setup();
    mount();
    act(() => exportController().open());
    const dialog = await screen.findByRole("dialog", { name: "Export video" });
    await user.click(within(dialog).getByRole("switch", { name: "Use hardware encoder" }));
    expect(within(dialog).getByText("Software only: slower, works on any computer.")).toBeTruthy();
    await user.click(within(dialog).getByRole("radio", { name: "Medium" }));
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() =>
      expect(call).toHaveBeenLastCalledWith(
        "export.start",
        expect.objectContaining({
          hw: "off",
          preset: {
            custom: {
              width: 1920,
              height: 1080,
              fps: { num: 30000, den: 1001 },
              quality: "medium",
            },
          },
        }),
      ),
    );
  });

  it("says why a value is refused and does not start", async () => {
    const user = userEvent.setup();
    mount();
    act(() => exportController().open());
    const dialog = await screen.findByRole("dialog", { name: "Export video" });
    await user.clear(within(dialog).getByLabelText("Width"));
    await user.type(within(dialog).getByLabelText("Width"), "1921");
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Width must be an even number from 16 to 8192.",
    );
    expect(call).not.toHaveBeenCalledWith("export.start", expect.anything());
  });

  it("cancels through the engine and reports it", async () => {
    const user = userEvent.setup();
    mount();
    act(() => exportController().open());
    await user.click(await screen.findByRole("button", { name: "Export" }));
    await waitFor(() => expect(exportStore.getState().active).not.toBeNull());
    act(() => jobStore.getState().applyProgress(running(0.2, "")));
    await user.click(await screen.findByRole("button", { name: "Cancel export" }));
    await waitFor(() => expect(call).toHaveBeenCalledWith("job.cancel", { id: JOB }));
    act(() =>
      jobStore
        .getState()
        .applyProgress({ ...running(0.2, ""), state: "cancelled", cancel_requested: true }),
    );
    expect(exportStore.getState().active).toBeNull();
    expect(toastStore.getState().notices[0]?.title).toBe("Export cancelled");
    expect(screen.queryByTestId("export-chip")).toBeNull();
  });
});
