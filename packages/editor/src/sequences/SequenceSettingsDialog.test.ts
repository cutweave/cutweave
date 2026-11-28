// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Sequence settings (WP-2B.03): the dialog over one sequence's format, and the background
// picker WP-2B.02 left for it: a still image of the bin with how it is fitted, and a transparent
// canvas. Every change is one catalog command inside one batch transaction (one undo step).
import type { Project, SequenceId } from "@cutweave/protocol";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../i18n/index.ts";
import { docStore } from "../state/docStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { sampleProject } from "../testing/fixtures.ts";
import { openSequenceSettings } from "./actions.ts";
import { backgroundImages, SequenceSettingsDialog } from "./SequenceSettingsDialog.tsx";
import { sequenceUiStore } from "./uiStore.ts";

const A = "seq_1" as SequenceId;

/** The fixture's project with two pictures (and its video) in the bin. */
function withImages(fill?: object): Project {
  const project = sampleProject();
  const video = Object.values(project.media)[0];
  const seq = project.sequences[A];
  if (video === undefined || seq === undefined) throw new Error("the fixture changed");
  const image = (id: string, file: string) => ({
    ...video,
    id,
    kind: "image",
    path_abs: `C:\\media\\${file}`,
  });
  return {
    ...project,
    media: {
      ...project.media,
      med_z: image("med_z", "wall.png"),
      med_b: image("med_b", "logo.png"),
    },
    sequences: {
      [A]: { ...seq, format: { ...seq.format, ...(fill && { background_fill: fill }) } },
    },
  } as unknown as Project;
}

const load = (project: Project, rev = 1): void =>
  docStore.getState().resync({ rev, committed_rev: rev, project });

let engine: FakeEngine;

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

beforeEach(() => {
  sequenceUiStore.setState({ renaming: null, settings: null });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engine = createFakeEngine();
  engine.install();
});

afterEach(() => {
  cleanup();
  engine.uninstall();
});

const mount = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SequenceSettingsDialog))));

const dialog = () => screen.getByRole("dialog", { name: "Sequence settings" });
const select = (name: string) => within(dialog()).getByLabelText(name) as HTMLSelectElement;
const options = (name: string) => [...select(name).options].map((o) => o.textContent);
/** The words of the chosen option. */
const chosen = (name: string) => select(name).selectedOptions[0]?.textContent;
const choose = async (name: string, label: string): Promise<void> => {
  const field = select(name);
  const option = [...field.options].find((o) => o.textContent === label);
  if (option === undefined) throw new Error(`${name} has no ${label}`);
  await act(async () => {
    fireEvent.change(field, { target: { value: option.value } });
  });
};
/** The document commands sent, without the transaction's begin and commit. */
const sent = () =>
  engine.calls.filter((c) => !c.name.startsWith("edit.")).map((c) => [c.name, c.input] as const);

describe("backgroundImages", () => {
  it("lists the bin's still images by name, never video or sound", () => {
    expect(backgroundImages(withImages().media)).toEqual([
      { id: "med_b", name: "logo.png" },
      { id: "med_z", name: "wall.png" },
    ]);
    expect(backgroundImages(sampleProject().media)).toEqual([]);
    expect(backgroundImages(undefined)).toEqual([]);
  });
});

describe("SequenceSettingsDialog", () => {
  it("is closed until asked for, and names the sequence it shows", () => {
    load(withImages());
    mount();
    expect(screen.queryByRole("dialog")).toBeNull();
    act(() => openSequenceSettings(A));
    expect(dialog().textContent).toContain("Main");
    expect(within(dialog()).getByRole("spinbutton", { name: "Width" })).toBeTruthy();
    expect(options("Reframe clips")).toEqual(["Fit", "Fill", "Keep scale", "None"]);
    fireEvent.click(within(dialog()).getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Fill offers the image and the transparent canvas; each choice is one sequence.set_background", async () => {
    load(withImages());
    mount();
    act(() => openSequenceSettings(A));
    expect(options("Fill")).toEqual(["Color only", "Blurred picture", "Image", "Transparent"]);
    await choose("Fill", "Transparent");
    expect(sent()).toEqual([["sequence.set_background", { sequence: A, kind: "transparent" }]]);
    // An image fill starts with the first picture by name.
    await choose("Fill", "Image");
    expect(sent().at(-1)).toEqual([
      "sequence.set_background",
      { sequence: A, kind: "image", media: "med_b" },
    ]);
    // One undo step each: a begin and a commit around every command.
    expect(engine.names().filter((n) => n === "edit.begin_tx")).toHaveLength(2);
    expect(engine.names().filter((n) => n === "edit.commit_tx")).toHaveLength(2);
  });

  it("with an image fill it lists the bin's pictures and how the image is fitted", async () => {
    load(withImages({ kind: "image", media: "med_b" }));
    mount();
    act(() => openSequenceSettings(A));
    expect(options("Image")).toEqual(["logo.png", "wall.png"]);
    expect(chosen("Image")).toBe("logo.png");
    expect(options("Fit")).toEqual(["Fill the frame", "Show all of it", "Stretch"]);
    expect(chosen("Fit")).toBe("Fill the frame");
    await choose("Image", "wall.png");
    expect(sent().at(-1)).toEqual([
      "sequence.set_background",
      { sequence: A, kind: "image", media: "med_z", fit: "cover" },
    ]);
    await choose("Fit", "Show all of it");
    expect(sent().at(-1)).toEqual([
      "sequence.set_background",
      { sequence: A, kind: "image", media: "med_b", fit: "contain" },
    ]);
    // The document is what shows: an agent's change (or an undo) arrives as a patch.
    act(() => load(withImages({ kind: "image", media: "med_z", fit: "stretch" }), 2));
    expect(chosen("Image")).toBe("wall.png");
    expect(chosen("Fit")).toBe("Stretch");
  });

  it("without a picture in the bin it says how to get one and offers no image fill", () => {
    load(sampleProject());
    mount();
    act(() => openSequenceSettings(A));
    expect(options("Fill")).toEqual(["Color only", "Blurred picture", "Transparent"]);
    expect(dialog().textContent).toContain("import it into the media bin first");
  });

  it("an image that left the bin stays listed as missing", () => {
    load(withImages({ kind: "image", media: "med_gone" }));
    mount();
    act(() => openSequenceSettings(A));
    expect(options("Image")).toEqual(["logo.png", "wall.png", "Missing image"]);
    expect(chosen("Image")).toBe("Missing image");
  });

  it("closes itself when its sequence is deleted", () => {
    load(withImages());
    mount();
    act(() => openSequenceSettings(A));
    expect(screen.getByRole("dialog")).toBeTruthy();
    const project = docStore.getState().project;
    if (project === null) throw new Error("no project");
    act(() => load({ ...project, sequences: {} } as Project, 2));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(sequenceUiStore.getState().settings).toBeNull();
  });
});
