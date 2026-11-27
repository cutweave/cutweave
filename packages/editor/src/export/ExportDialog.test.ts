// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-1M.08, WP-2G.04: the export dialog and the top bar's chip, over the real stores and a
// recording engine that answers like the real one (presets, resolve, encoders, options). The
// user flows F14 and F34 run the same dialog in the app on the real engine.
import type { ExportPresetEntry, ExportPresetSpec, JobInfo, Project } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { healthStore } from "../media/health.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { initialSessionState, sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { jobStore } from "../state/jobStore.ts";
import { toastStore } from "../state/toastStore.ts";
import {
  ENCODERS,
  exportProject,
  H264_OPTIONS,
  PRESET_ENTRIES,
  resolveFor,
  USER_PRESET,
} from "../testing/exportFixtures.ts";
import { ExportStatusChip } from "./ExportChip.tsx";
import { ExportDialog } from "./ExportDialog.tsx";
import { exportStore, initialExportState } from "./exportStore.ts";
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
const F = 705_600_000;

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
let seq: { width: number; height: number; seconds: number };

const mount = () =>
  render(
    h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(ExportDialog), h(ExportStatusChip))),
  );

function useProject(project: Project, size: typeof seq): void {
  seq = size;
  docStore.setState({ project } as never);
}

beforeEach(async () => {
  await preferences.resetAll();
  resetExportController();
  call = vi.fn(async (name: string, input: Record<string, unknown>) => {
    if (name === "export.list_presets") return { presets: PRESET_ENTRIES, problems: [] };
    if (name === "export.list_encoders") return { codecs: ENCODERS };
    if (name === "export.describe_options")
      return { codec: "h264", encoder: "libx264", hardware: false, schema: H264_OPTIONS };
    if (name === "export.resolve_preset") return resolveFor(String(input.preset), seq);
    if (name === "export.start" || name === "export.enqueue") {
      return { job_id: JOB, path: String(input.path), warnings: [], estimated_kib: null };
    }
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
  useProject(exportProject(1920, 1080, 10), { width: 1920, height: 1080, seconds: 10 });
  jobStore.setState({ jobs: {} });
  exportStore.setState({ ...initialExportState });
  toastStore.getState().clear();
  exportController().attach();
});

afterEach(() => {
  cleanup();
  resetExportController();
});

async function openDialog() {
  mount();
  act(() => exportController().open());
  return settings();
}

/** The settings dialog once the presets are there (the loading dialog is another element). */
async function settings() {
  await screen.findByLabelText("Preset");
  return screen.getByRole("dialog", { name: "Export video" });
}

const presetSelect = (dialog: HTMLElement) =>
  within(dialog).getByLabelText("Preset") as HTMLSelectElement;
const lastCall = (name: string) =>
  call.mock.calls.filter(([n]) => n === name).at(-1)?.[1] as Record<string, unknown> | undefined;

/**
 * Makes the engine refuse the next export that does not allow offline media, the way it does
 * when `text` names missing files (other calls, like the preset's resolve, go through).
 */
function refuseOffline(text: string): void {
  const answer = call.getMockImplementation() as
    | ((name: string, input: Record<string, unknown>) => Promise<unknown>)
    | undefined;
  let refused = false;
  call.mockImplementation(async (name: string, input: Record<string, unknown>) => {
    const starts = name === "export.start" || name === "export.enqueue";
    if (starts && !refused && input.allow_offline !== true) {
      refused = true;
      throw new RpcError("invalid", text, "/media/med_1");
    }
    return answer?.(name, input);
  });
}

/** Makes the engine refuse the next `export.start` or `export.enqueue`, and only that call. */
function refuseStart(error: RpcError): void {
  const answer = call.getMockImplementation() as
    | ((name: string, input: Record<string, unknown>) => Promise<unknown>)
    | undefined;
  let refused = false;
  call.mockImplementation(async (name: string, input: Record<string, unknown>) => {
    if ((name === "export.start" || name === "export.enqueue") && !refused) {
      refused = true;
      throw error;
    }
    return answer?.(name, input);
  });
}

describe("ExportDialog", () => {
  it("on a 9:16 sequence puts the portrait platform presets first; Reels shows its size and limit", async () => {
    useProject(exportProject(1080, 1920, 200), { width: 1080, height: 1920, seconds: 200 });
    const user = userEvent.setup();
    const dialog = await openDialog();
    const groups = [...presetSelect(dialog).querySelectorAll("optgroup")];
    expect(groups[0]?.label).toBe("For this 9:16 sequence");
    expect([...(groups[0]?.querySelectorAll("option") ?? [])].map((o) => o.textContent)).toEqual([
      "YouTube Shorts",
      "Instagram Reels",
      "TikTok",
      "X portrait",
    ]);
    expect(groups.map((g) => g.label)).toEqual([
      "For this 9:16 sequence",
      "YouTube",
      "Instagram",
      "X",
      "General",
    ]);
    // GIF is listed, off, saying it is not in this build.
    const gif = within(dialog).getByRole("option", { name: "GIF (not in this build)" });
    expect((gif as HTMLOptionElement).disabled).toBe(true);

    await user.selectOptions(presetSelect(dialog), "instagram-reels");
    const summary = within(dialog).getByTestId("export-summary");
    await waitFor(() =>
      expect(summary.textContent).toContain("1080 × 1920 · 30 fps · H.264 + AAC · MP4"),
    );
    expect(summary.textContent).toContain("Length 00:03:20:00");
    expect(summary.textContent).toContain("about");
    expect(
      await within(dialog).findByText(
        "Instagram Reels takes videos up to 3:00; this export is 3:20. You can still export it.",
      ),
    ).toBeTruthy();
    expect(within(dialog).getByText("1080 x 1920, up to 30 fps")).toBeTruthy();
    // Over the limit is a warning, never a block.
    expect(within(dialog).getByRole("button", { name: "Export" }).hasAttribute("disabled")).toBe(
      false,
    );

    // YouTube 1080p on the same sequence says it fits with bars.
    await user.selectOptions(presetSelect(dialog), "youtube-1080p");
    expect(
      await within(dialog).findByText(
        /^Fits with bars: YouTube 1080p is 16:9 and this sequence is 9:16/,
      ),
    ).toBeTruthy();
  });

  it("starts at the general preset, the project's folder and the default quality (Settings > Export)", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    expect(presetSelect(dialog).value).toBe("h264-mp4-1080p");
    expect(within(dialog).getByLabelText("Save to")).toHaveProperty(
      "value",
      "C:\\p\\Film\\Film.mp4",
    );
    await user.click(within(dialog).getByRole("button", { name: "Advanced" }));
    await waitFor(() =>
      expect(within(dialog).getByLabelText("Width")).toHaveProperty("value", "1920"),
    );
    expect(within(dialog).getByLabelText("Height")).toHaveProperty("value", "1080");
    expect(within(dialog).getByLabelText("Frame rate")).toHaveProperty("value", "30");
    expect(within(dialog).getByRole("radio", { name: "Medium" }).getAttribute("aria-checked")).toBe(
      "true",
    );
    expect(
      await within(dialog).findByText("Found NVIDIA NVENC. Faster than software."),
    ).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(lastCall("export.start")).toBeDefined());
    expect(lastCall("export.start")).toEqual({
      sequence: "seq_1",
      path: "C:\\p\\Film\\Film.mp4",
      preset: { custom: { quality: "medium", preset: "h264-mp4-1080p" } },
      hw: "auto",
      overwrite: false,
      range: { kind: "sequence" },
    });
    expect(await preferences.get("export.last_preset")).toBe("h264-mp4-1080p");
  });

  it("sends a platform preset as itself, and the size typed in Advanced on top of it", async () => {
    await preferences.set("export.hw", "off");
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.selectOptions(presetSelect(dialog), "youtube-4k");
    await user.click(within(dialog).getByRole("button", { name: "Advanced" }));
    expect(within(dialog).getByText("40 Mb/s, set by the preset")).toBeTruthy();
    expect(within(dialog).getByText("Software only: slower, works on any computer.")).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(lastCall("export.start")).toBeDefined());
    expect(lastCall("export.start")).toMatchObject({ preset: "youtube-4k", hw: "off" });
    act(() => jobStore.getState().applyProgress({ ...running(1, ""), state: "succeeded" }));

    act(() => exportController().open());
    const again = await settings();
    // The last preset used is where the next export starts.
    expect(presetSelect(again).value).toBe("youtube-4k");
    await user.click(within(again).getByRole("button", { name: "Advanced" }));
    await user.clear(within(again).getByLabelText("Width"));
    await user.type(within(again).getByLabelText("Width"), "1921");
    await user.click(within(again).getByRole("button", { name: "Export" }));
    expect((await within(again).findByRole("alert")).textContent).toBe(
      "Width must be an even number from 16 to 8192.",
    );
    await user.clear(within(again).getByLabelText("Width"));
    await user.type(within(again).getByLabelText("Width"), "2560");
    await user.clear(within(again).getByLabelText("Height"));
    await user.type(within(again).getByLabelText("Height"), "1440");
    await user.click(within(again).getByRole("button", { name: "Export" }));
    await waitFor(() =>
      expect(lastCall("export.start")).toMatchObject({
        preset: { custom: { width: 2560, height: 1440, preset: "youtube-4k" } },
      }),
    );
  });

  it("gives ProRes its .mov and checks the extension", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.selectOptions(presetSelect(dialog), "prores-422-hq");
    const path = within(dialog).getByLabelText("Save to") as HTMLInputElement;
    expect(path.value).toBe("C:\\p\\Film\\Film.mov");
    await user.clear(path);
    await user.type(path, "C:\\p\\Film\\Film.mp4");
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "The file name must end in .mov.",
    );
    expect(lastCall("export.start")).toBeUndefined();
  });

  it("exports a custom range, and says when the times do not make one", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    // No in and out points, no markers: those choices are off.
    expect(within(dialog).getByRole("radio", { name: "In to out" }).hasAttribute("disabled")).toBe(
      true,
    );
    expect(within(dialog).getByRole("radio", { name: "Markers" }).hasAttribute("disabled")).toBe(
      true,
    );
    await user.click(within(dialog).getByRole("radio", { name: "Custom" }));
    await user.type(within(dialog).getByLabelText("Start"), "00:00:02:00");
    await user.type(within(dialog).getByLabelText("End"), "1s");
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "Type a start and an end inside the sequence, the end after the start.",
    );
    await user.clear(within(dialog).getByLabelText("End"));
    await user.type(within(dialog).getByLabelText("End"), "00:00:06:00");
    await waitFor(() =>
      expect(within(dialog).getByTestId("export-summary").textContent).toContain(
        "Length 00:00:04:00",
      ),
    );
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() =>
      expect(lastCall("export.start")).toMatchObject({
        range: { kind: "custom", in: "00:00:02:00", out: "00:00:06:00" },
      }),
    );
  });

  it("exports one file per marker under a name template", async () => {
    const markers = {
      m1: { id: "m1", t: 0, duration: 2 * F, label: "Intro", color: "#fff", kind: "user" },
      m2: { id: "m2", t: 4 * F, duration: 2 * F, label: "Hook", color: "#fff", kind: "user" },
    };
    useProject(exportProject(1920, 1080, 10, { markers }), {
      width: 1920,
      height: 1080,
      seconds: 2,
    });
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("radio", { name: "Markers" }));
    expect(within(dialog).getByText("Makes 2 files, one per marker.")).toBeTruthy();
    expect(within(dialog).getByLabelText("Save to")).toHaveProperty(
      "value",
      "C:\\p\\Film\\Film_{marker_label}_{n}.mp4",
    );
    await user.click(within(dialog).getByRole("radio", { name: "To the next marker" }));
    expect(within(dialog).getByText("Makes 1 file.")).toBeTruthy();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() =>
      expect(lastCall("export.start")).toMatchObject({
        path: "C:\\p\\Film\\Film_{marker_label}_{n}.mp4",
        range: { kind: "markers", pair_mode: "pairs" },
      }),
    );
  });

  it("shows the engine's refusal at the field it names: an encoder option, the path", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Advanced" }));
    const form = await within(dialog).findByRole("group", { name: "Encoder settings" });
    await user.selectOptions(within(form).getByLabelText("Mode"), "CBR");
    call.mockRejectedValueOnce(
      new RpcError(
        "invalid",
        "cbr needs a bitrate_kbps; set one",
        "/advanced/video/rate_control/mode",
      ),
    );
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    const alert = await within(form).findByRole("alert");
    expect(alert.textContent).toBe("cbr needs a bitrate_kbps; set one");
    expect(alert.getAttribute("data-path")).toBe("video.rate_control.mode");
    expect(lastCall("export.start")).toMatchObject({
      advanced: { video: { rate_control: { mode: "cbr" } } },
    });
    // Back to the preset's settings: nothing extra is sent.
    await user.click(within(dialog).getByRole("button", { name: "Use the preset's" }));
    call.mockRejectedValueOnce(
      new RpcError("invalid", "not enough room: 2 GB needed, 1 GB free", "/path"),
    );
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    expect((await within(dialog).findByRole("alert")).textContent).toBe(
      "not enough room: 2 GB needed, 1 GB free",
    );
    expect(lastCall("export.start")?.advanced).toBeUndefined();
    expect(within(dialog).getByLabelText("Save to").getAttribute("aria-invalid")).toBe("true");
  });

  it("asks before replacing a file unless told what to do; Replace sends overwrite", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    call.mockRejectedValueOnce(new RpcError("invalid", "exists", "/overwrite"));
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    const replace = await screen.findByRole("dialog", { name: "Replace Film.mp4?" });
    await user.click(within(replace).getByRole("button", { name: "Replace" }));
    await waitFor(() => expect(lastCall("export.start")).toMatchObject({ collision: "overwrite" }));

    act(() => jobStore.getState().applyProgress({ ...running(1, ""), state: "succeeded" }));
    act(() => exportController().open());
    const again = await settings();
    await user.selectOptions(within(again).getByLabelText("If the file exists"), "rename");
    await user.click(within(again).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(lastCall("export.start")).toMatchObject({ collision: "rename" }));
  });

  it("says media files are missing, with Relink… and Export anyway (allow_offline)", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    refuseOffline(
      "1 media file is missing: `beach.mp4`; relink them or pass `allow_offline: true`",
    );
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    const notice = await within(dialog).findByTestId("export-offline");
    expect(notice.getAttribute("role")).toBe("alert");
    expect(notice.textContent).toContain(
      'beach.mp4 is missing. Its clips would show a "Media offline" card in the video and stay silent.',
    );
    expect(lastCall("export.start")).not.toHaveProperty("allow_offline");
    // The engine's own sentence (commands and parameters) is not shown.
    expect(dialog.textContent).not.toContain("allow_offline");

    // Export anyway: the same request again, with the slate allowed.
    await user.click(within(notice).getByRole("button", { name: "Export anyway" }));
    await waitFor(() => expect(lastCall("export.start")).toMatchObject({ allow_offline: true }));
    expect(exportStore.getState().active).not.toBeNull();

    // Several files are counted; Relink… closes the dialog and opens the relink dialog.
    act(() => jobStore.getState().applyProgress({ ...running(1, ""), state: "succeeded" }));
    act(() => exportController().open());
    const again = await settings();
    refuseOffline("3 media files are missing: `a.mp4`, `b.mp4`, `c.wav`; relink them");
    await user.click(within(again).getByRole("button", { name: "Add to queue" }));
    const many = await within(again).findByTestId("export-offline");
    expect(many.textContent).toContain("3 media files are missing.");
    expect(healthStore.getState().dialogOpen).toBe(false);
    await user.click(within(many).getByRole("button", { name: "Relink…" }));
    expect(exportStore.getState().dialogOpen).toBe(false);
    expect(healthStore.getState().dialogOpen).toBe(true);
    act(() => healthStore.getState().closeDialog());
  });

  it("keeps Replace when the file exists and media is missing", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    // The refusals are the answers to the export itself. (Refusing "the next call" instead
    // also catches the preset's resolve or the encoder list when one of them is still on its
    // way, and then the export goes through: that made this test fail on a busy machine.)
    refuseStart(new RpcError("invalid", "exists", "/overwrite"));
    await user.click(within(dialog).getByRole("button", { name: "Add to queue" }));
    const replace = await screen.findByRole("dialog", { name: "Replace Film.mp4?" });
    refuseStart(
      new RpcError("invalid", "1 media file is missing: `beach.mp4`; relink", "/media/med_1"),
    );
    await user.click(within(replace).getByRole("button", { name: "Replace" }));
    const back = await settings();
    const notice = await within(back).findByTestId("export-offline");
    await user.click(within(notice).getByRole("button", { name: "Export anyway" }));
    await waitFor(() =>
      expect(lastCall("export.enqueue")).toMatchObject({
        allow_offline: true,
        collision: "overwrite",
      }),
    );
  });

  it("adds to the queue: export.enqueue, the dialog closes, a toast says so", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Add to queue" }));
    await waitFor(() => expect(lastCall("export.enqueue")).toBeDefined());
    expect(lastCall("export.start")).toBeUndefined();
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(toastStore.getState().notices[0]?.title).toBe("Added to the export queue");
    expect(exportStore.getState().active).toBeNull();
  });

  it("shows the progress, the chip, and the hide and cancel", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(exportStore.getState().active).not.toBeNull());
    act(() =>
      jobStore.getState().applyProgress(running(0.4, "frame 12 of 30, 20.5 fps, about 0:03 left")),
    );
    const progress = await screen.findByRole("dialog", { name: "Exporting Film.mp4" });
    expect(within(progress).getByRole("progressbar").getAttribute("aria-valuenow")).toBe("40");
    expect(within(progress).getByText("20.5 fps · about 0:03 left")).toBeTruthy();
    expect(screen.getByTestId("export-chip").textContent).toBe("Exporting 40%");
    await user.click(within(progress).getByRole("button", { name: "Hide" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    await user.click(screen.getByTestId("export-chip"));
    const back = await screen.findByRole("dialog", { name: "Exporting Film.mp4" });
    await user.click(within(back).getByRole("button", { name: "Cancel export" }));
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

  it("has no axe violations, with Advanced open and a warning shown", async () => {
    useProject(exportProject(1080, 1920, 200), { width: 1080, height: 1920, seconds: 200 });
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.selectOptions(presetSelect(dialog), "instagram-reels");
    await within(dialog).findByText(/takes videos up to 3:00/);
    await user.click(within(dialog).getByRole("button", { name: "Advanced" }));
    await within(dialog).findByRole("group", { name: "Encoder settings" });
    expect((await axe.run(dialog)).violations).toEqual([]);
  });
});

describe("ExportDialog: presets of your own (WP-2G.04b)", () => {
  /** An engine that keeps saved presets, as the real one does. */
  function keepPresets(start: readonly ExportPresetEntry[] = PRESET_ENTRIES) {
    let entries = [...start];
    const answer = call.getMockImplementation() as (
      name: string,
      input: Record<string, unknown>,
    ) => Promise<unknown>;
    call.mockImplementation(async (name: string, input: Record<string, unknown>) => {
      if (name === "export.list_presets") return { presets: entries, problems: [] };
      if (name === "export.save_preset") {
        const preset = input.preset as ExportPresetSpec;
        const replaced = entries.some((e) => e.preset.id === preset.id);
        entries = [
          ...entries.filter((e) => e.preset.id !== preset.id),
          { source: "user", supported: true, preset },
        ];
        return { id: preset.id, replaced };
      }
      if (name === "export.delete_preset") {
        entries = entries.filter((e) => e.preset.id !== input.id);
        return { id: input.id };
      }
      if (name === "export.resolve_preset") return resolveFor("h264-mp4-1080p", seq);
      return answer(name, input);
    });
  }

  it("saves what the dialog shows as a preset under a name, and chooses it", async () => {
    keepPresets();
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Advanced" }));
    await user.click(await within(dialog).findByRole("radio", { name: "Low" }));
    const width = within(dialog).getByLabelText("Width");
    await user.clear(width);
    await user.type(width, "1280");

    await user.click(within(dialog).getByRole("button", { name: "Save as preset…" }));
    const name = within(dialog).getByLabelText("Preset name");
    // Enter with nothing typed asks for a name; it neither saves nor starts the export.
    await user.type(name, "{Enter}");
    expect(await within(dialog).findByText("Type a name for the preset.")).toBeTruthy();
    expect(lastCall("export.save_preset")).toBeUndefined();
    await user.type(name, "Review copy{Enter}");
    await waitFor(() => expect(lastCall("export.save_preset")).toBeDefined());
    expect(lastCall("export.start")).toBeUndefined();
    const saved = lastCall("export.save_preset")?.preset as ExportPresetSpec;
    expect(saved).toMatchObject({
      schema_version: 1,
      id: "review-copy",
      name: "Review copy",
      summary: "Fit into 1920 x 1080, sequence frame rate, high quality",
      container: "mp4",
      size: { kind: "fixed", width: 1280, height: 1080 },
      fps: { kind: "follow_sequence" },
    });
    expect(saved.video).toMatchObject({ codec: "h264", rate_control: { mode: "crf", crf: 28 } });

    // The picker is on the new preset, under "Your presets"; the name row is gone.
    await waitFor(() => expect(presetSelect(dialog).value).toBe("review-copy"));
    const mine = presetSelect(dialog).querySelector('optgroup[label="Your presets"]');
    expect([...(mine?.querySelectorAll("option") ?? [])].map((o) => o.textContent)).toEqual([
      "Review copy",
    ]);
    expect(within(dialog).queryByLabelText("Preset name")).toBeNull();
    expect(toastStore.getState().notices.at(-1)?.title).toBe("Preset saved");
    // Exporting now sends the preset by its id, with no changes on top.
    await user.click(within(dialog).getByRole("button", { name: "Export" }));
    await waitFor(() => expect(lastCall("export.start")).toBeDefined());
    expect(lastCall("export.start")?.preset).toBe("review-copy");
  });

  it("says which preset of yours the name replaces, and why the engine refused", async () => {
    keepPresets([...PRESET_ENTRIES, USER_PRESET]);
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Save as preset…" }));
    await user.type(within(dialog).getByLabelText("Preset name"), "my review copy");
    expect(within(dialog).getByText("This replaces your preset “My review copy”.")).toBeTruthy();
    const answer = call.getMockImplementation() as (n: string, i: unknown) => Promise<unknown>;
    call.mockImplementation(async (name: string, input: unknown) => {
      if (name === "export.save_preset") {
        throw new RpcError("invalid", "at most 200 user presets; delete one first", "/preset");
      }
      return answer(name, input);
    });
    await user.click(within(dialog).getByRole("button", { name: "Save preset" }));
    expect(
      await within(dialog).findByText(
        "The preset was not saved: at most 200 user presets; delete one first",
      ),
    ).toBeTruthy();
    expect(lastCall("export.save_preset")?.preset).toMatchObject({ id: "my-review" });
    // Don't save: the row closes and nothing else changed.
    await user.click(within(dialog).getByRole("button", { name: "Don’t save the preset" }));
    expect(within(dialog).queryByLabelText("Preset name")).toBeNull();
    expect(presetSelect(dialog).value).toBe("h264-mp4-1080p");
  });

  it("deletes a preset of yours, never a built-in one, and Undo brings it back", async () => {
    keepPresets([...PRESET_ENTRIES, USER_PRESET]);
    const user = userEvent.setup();
    const dialog = await openDialog();
    expect(within(dialog).queryByRole("button", { name: "Delete this preset" })).toBeNull();
    await user.selectOptions(presetSelect(dialog), "my-review");
    await user.click(within(dialog).getByRole("button", { name: "Delete this preset" }));
    await waitFor(() => expect(lastCall("export.delete_preset")).toEqual({ id: "my-review" }));
    await waitFor(() => expect(presetSelect(dialog).value).toBe("h264-mp4-1080p"));
    expect(presetSelect(dialog).querySelector('optgroup[label="Your presets"]')).toBeNull();
    const notice = toastStore.getState().notices.at(-1);
    expect(notice?.title).toBe("Preset deleted");
    act(() => notice?.actions?.[0]?.onClick());
    await waitFor(() =>
      expect(presetSelect(dialog).querySelector('optgroup[label="Your presets"]')).not.toBeNull(),
    );
  });

  it("has no accessibility violations with the name row open", async () => {
    const user = userEvent.setup();
    const dialog = await openDialog();
    await user.click(within(dialog).getByRole("button", { name: "Save as preset…" }));
    await user.type(within(dialog).getByLabelText("Preset name"), "{Enter}");
    const results = await axe.run(dialog, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations.map((v) => v.id)).toEqual([]);
  });
});
