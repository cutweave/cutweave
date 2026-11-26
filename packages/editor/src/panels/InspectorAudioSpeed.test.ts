// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The Inspector's Audio, Speed and Sequence sections (WP-2H.03b): each is data-driven or built on
// the same editor as the Video sections, several clips edit as one command, and the sections that
// do not apply (sound for a picture clip, speed for a freeze frame) are not shown.
import type { Project } from "@cutweave/protocol";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { sampleProject, sec } from "../testing/fixtures.ts";
import { InspectorPanel } from "./InspectorPanel.tsx";

let call: ReturnType<typeof vi.fn>;

function connect(): void {
  call = vi.fn(async (name: string) => {
    if (name === "edit.begin_tx") return { tx: "tx_1" };
    if (name === "media.proxy_status") return { proxies: [] };
    if (name === "keyframe.eval") return { samples: [] };
    return {};
  });
  engineStore.setState({
    rpc: { call, on: () => () => undefined } as never,
    connection: "ready",
    blocked: null,
  });
}

const calls = (command: string) =>
  call.mock.calls.filter(([name]) => name === command).map(([, input]) => input);

const view = () => render(h(I18nextProvider, { i18n }, h(InspectorPanel)));

type Edits = Record<string, object>;

/** The sample project with a second sound clip `itm_a2` next to `itm_a` on the audio track. */
function withSecondSound(edits: Edits = {}): Project {
  const project = sampleProject(edits);
  const track = (project.sequences as unknown as Record<string, unknown>).seq_1 as {
    tracks: { trk_a: { items: Record<string, object> } };
  };
  const first = track.tracks.trk_a.items.itm_a as { id: string };
  track.tracks.trk_a.items.itm_a2 = { ...structuredClone(first), id: "itm_a2" };
  return project as Project;
}

function select(project: Project, playhead: number, ...ids: string[]): void {
  docStore.setState({ project, status: "ready" } as never);
  viewStore.setState({
    selection: ids.map((id) => ({ kind: "item", id })),
    playhead,
  } as never);
}

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(connect);
afterEach(cleanup);

describe("the Audio section", () => {
  it("shows volume, pan and fades for a sound clip, and none for a picture clip", () => {
    select(sampleProject(), sec(2), "itm_a");
    view();
    expect(screen.getByRole("button", { name: "Audio" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Volume" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Pan" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Fade in" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Fade out" })).toBeTruthy();
    expect(screen.getByLabelText("Fade shape")).toBeTruthy();
    // The hand-coded Gain row is gone; the engine has no preserve pitch or normalize yet.
    expect(screen.queryByRole("spinbutton", { name: "Gain" })).toBeNull();
    expect(screen.queryByText(/pitch|normalize/i)).toBeNull();
    cleanup();
    select(sampleProject(), sec(2), "itm_v");
    view();
    expect(screen.queryByRole("button", { name: "Audio" })).toBeNull();
  });

  it("sets volume for two sound clips as one command in one undo step", async () => {
    select(withSecondSound(), sec(2), "itm_a", "itm_a2");
    view();
    const user = userEvent.setup();
    await user.dblClick(screen.getByRole("spinbutton", { name: "Volume" }));
    await user.keyboard("-6{Enter}");
    await waitFor(() => expect(calls("item.set_props")).toHaveLength(1));
    const [input] = calls("item.set_props") as Array<{
      items: string[];
      props: { audio_gain: number; audio_pan: null; opacity: null };
      at: number;
    }>;
    expect(input?.items).toEqual(["itm_a", "itm_a2"]);
    expect(input?.props.audio_gain).toBe(-6);
    expect(input?.props.audio_pan).toBeNull();
    expect(input?.at).toBe(sec(2));
    expect(calls("edit.begin_tx")).toEqual([{ kind: "batch", label: "Change volume" }]);
    expect(calls("edit.commit_tx")).toHaveLength(1);
  });

  it("sets pan for two sound clips as one command", async () => {
    select(withSecondSound(), sec(2), "itm_a", "itm_a2");
    view();
    const user = userEvent.setup();
    await user.dblClick(screen.getByRole("spinbutton", { name: "Pan" }));
    await user.keyboard("-0.5{Enter}");
    await waitFor(() => expect(calls("item.set_props")).toHaveLength(1));
    const [input] = calls("item.set_props") as Array<{
      items: string[];
      props: { audio_pan: number; audio_gain: null };
    }>;
    expect(input?.items).toEqual(["itm_a", "itm_a2"]);
    expect(input?.props.audio_pan).toBe(-0.5);
    expect(input?.props.audio_gain).toBeNull();
  });

  it("edits only the sound clips of a mixed selection", async () => {
    select(sampleProject(), sec(2), "itm_v", "itm_a");
    view();
    const user = userEvent.setup();
    await user.dblClick(screen.getByRole("spinbutton", { name: "Volume" }));
    await user.keyboard("3{Enter}");
    await waitFor(() => expect(calls("item.set_props")).toHaveLength(1));
    expect((calls("item.set_props")[0] as { items: string[] }).items).toEqual(["itm_a"]);
  });

  it("keys volume at the playhead from the diamond", async () => {
    select(sampleProject(), sec(2), "itm_a");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Add keyframe for Volume" }));
    await waitFor(() => expect(calls("keyframe.set")).toHaveLength(1));
    expect(calls("keyframe.set")[0]).toMatchObject({
      items: ["itm_a"],
      prop: "audio_gain",
      keys: [{ at: sec(2), t: null, value: null }],
    });
  });

  it("sends a typed fade in seconds to audio.set_fade for every selected clip", async () => {
    select(withSecondSound(), sec(2), "itm_a", "itm_a2");
    view();
    const user = userEvent.setup();
    await user.dblClick(screen.getByRole("spinbutton", { name: "Fade in" }));
    await user.keyboard("0.5{Enter}");
    await waitFor(() => expect(calls("audio.set_fade")).toHaveLength(1));
    expect(calls("audio.set_fade")[0]).toEqual({ items: ["itm_a", "itm_a2"], in: "0.5s" });
    expect(calls("edit.begin_tx")).toEqual([{ kind: "batch", label: "Change fade in" }]);
  });

  it("shows a fade the clip has and changes its shape", async () => {
    const fade = { duration: sec(1), curve: "log" };
    select(
      sampleProject({ itm_a: { audio: { gain: 0, pan: 0, fade_in: fade } } }),
      sec(2),
      "itm_a",
    );
    view();
    expect(screen.getByRole("spinbutton", { name: "Fade in" }).getAttribute("aria-valuenow")).toBe(
      "1",
    );
    const user = userEvent.setup();
    await user.selectOptions(screen.getByLabelText("Fade shape"), "Equal power");
    await waitFor(() => expect(calls("audio.set_fade")).toHaveLength(1));
    expect(calls("audio.set_fade")[0]).toEqual({ items: ["itm_a"], curve: "equal_power" });
  });

  it("resets volume, pan and fades with the section's reset (group audio)", async () => {
    select(sampleProject({ itm_a: { audio: { gain: -6, pan: 0 } } }), sec(2), "itm_a");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: /Reset Audio/i }));
    await waitFor(() => expect(calls("item.reset_props")).toHaveLength(1));
    expect(calls("item.reset_props")[0]).toEqual({ items: ["itm_a"], group: "audio" });
  });
});

describe("the Speed section", () => {
  it("is hidden for sound and for a freeze frame", () => {
    select(sampleProject(), sec(2), "itm_a");
    view();
    expect(screen.queryByRole("button", { name: "Speed" })).toBeNull();
    cleanup();
    select(sampleProject({ itm_v: { freeze: { at_source: sec(1) } } }), sec(2), "itm_v");
    view();
    expect(screen.queryByRole("button", { name: "Speed" })).toBeNull();
  });

  it("shows the speed as text and sends what is typed to clip.set_speed unchanged", async () => {
    select(sampleProject({ itm_v: { speed: { num: 1, den: 2 } } }), sec(2), "itm_v");
    view();
    const field = screen.getByRole("textbox", { name: "Speed" }) as HTMLInputElement;
    expect(field.value).toBe("0.5x");
    const user = userEvent.setup();
    await user.clear(field);
    await user.type(field, "1/3{Enter}");
    await waitFor(() => expect(calls("clip.set_speed")).toHaveLength(1));
    expect(calls("clip.set_speed")[0]).toEqual({
      items: ["itm_v"],
      speed: "1/3",
      keep: "in_point",
      keyframes: "keep",
      ripple: false,
    });
    expect(calls("edit.begin_tx")).toEqual([{ kind: "batch", label: "Change speed" }]);
  });

  it("sends the options the person chose with the speed", async () => {
    select(sampleProject(), sec(2), "itm_v");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("radio", { name: "Stays" }));
    await user.click(screen.getByRole("switch", { name: "Scale keyframes" }));
    await user.click(screen.getByRole("switch", { name: "Move what follows" }));
    const field = screen.getByRole("textbox", { name: "Speed" });
    await user.clear(field);
    await user.type(field, "2x{Enter}");
    await waitFor(() => expect(calls("clip.set_speed")).toHaveLength(1));
    expect(calls("clip.set_speed")[0]).toEqual({
      items: ["itm_v"],
      speed: "2x",
      keep: "duration",
      keyframes: "scale",
      ripple: true,
    });
  });

  it("shows the engine's refusal for a speed it does not accept", async () => {
    select(sampleProject(), sec(2), "itm_v");
    call.mockImplementation(async (name: string) => {
      if (name === "edit.begin_tx") return { tx: "tx_1" };
      if (name === "clip.set_speed") {
        throw Object.assign(new Error("speed out of range"), {
          code: -32001,
          data: { kind: "Invalid", pointer: "/speed" },
        });
      }
      if (name === "media.proxy_status") return { proxies: [] };
      return {};
    });
    view();
    const user = userEvent.setup();
    const field = screen.getByRole("textbox", { name: "Speed" });
    await user.clear(field);
    await user.type(field, "500x{Enter}");
    await waitFor(() => expect(calls("edit.cancel_tx")).toHaveLength(1));
    expect(calls("edit.commit_tx")).toHaveLength(0);
  });

  it("reverses and resets", async () => {
    select(sampleProject({ itm_v: { speed: { num: 2, den: 1 } } }), sec(2), "itm_v");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("switch", { name: "Reverse" }));
    await waitFor(() => expect(calls("clip.reverse")).toHaveLength(1));
    expect(calls("clip.reverse")[0]).toEqual({ items: ["itm_v"], reverse: true });
    await user.click(screen.getByRole("button", { name: "Reset Speed" }));
    await waitFor(() => expect(calls("clip.reset_speed")).toHaveLength(1));
    expect(calls("clip.reset_speed")[0]).toEqual({
      items: ["itm_v"],
      keyframes: "keep",
      ripple: false,
    });
  });

  it("freezes the frame at the playhead, for one clip with the playhead inside it", async () => {
    select(sampleProject(), sec(2), "itm_v");
    view();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Freeze frame at playhead" }));
    await waitFor(() => expect(calls("clip.freeze_frame")).toHaveLength(1));
    expect(calls("clip.freeze_frame")[0]).toEqual({ item: "itm_v", at: sec(2) });
    cleanup();
    select(sampleProject(), sec(9), "itm_v");
    view();
    expect(
      (screen.getByRole("button", { name: "Freeze frame at playhead" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
  });
});

describe("the Sequence section", () => {
  const nothing = (): void => select(sampleProject(), sec(2));

  it("shows the format of the active sequence when nothing is selected", () => {
    nothing();
    view();
    expect(screen.getByRole("button", { name: "Sequence" })).toBeTruthy();
    expect(screen.getByRole("spinbutton", { name: "Width" }).getAttribute("aria-valuenow")).toBe(
      "1920",
    );
    expect(screen.getByRole("spinbutton", { name: "Height" }).getAttribute("aria-valuenow")).toBe(
      "1080",
    );
    expect(
      (screen.getByLabelText("Frame rate") as HTMLSelectElement).selectedOptions[0]?.textContent,
    ).toBe("30 fps");
  });

  it("sets the canvas width with the chosen reframe, as one command", async () => {
    nothing();
    view();
    const user = userEvent.setup();
    await user.selectOptions(screen.getByLabelText("Reframe clips"), "Fill");
    await user.dblClick(screen.getByRole("spinbutton", { name: "Width" }));
    await user.keyboard("1280{Enter}");
    await waitFor(() => expect(calls("sequence.set_format")).toHaveLength(1));
    expect(calls("sequence.set_format")[0]).toEqual({
      sequence: "seq_1",
      width: 1280,
      reframe: "fill",
    });
    expect(calls("edit.begin_tx")).toEqual([{ kind: "batch", label: "Change sequence width" }]);
  });

  it("changes the frame rate", async () => {
    nothing();
    view();
    const user = userEvent.setup();
    await user.selectOptions(screen.getByLabelText("Frame rate"), "24 fps");
    await waitFor(() => expect(calls("sequence.set_format")).toHaveLength(1));
    expect(calls("sequence.set_format")[0]).toEqual({
      sequence: "seq_1",
      frame_rate: { num: 24, den: 1 },
    });
  });

  it("is not shown while a clip is selected", () => {
    select(sampleProject(), sec(2), "itm_v");
    view();
    expect(screen.queryByTestId("sequence-inspector")).toBeNull();
  });
});
