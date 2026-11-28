// SPDX-License-Identifier: GPL-3.0-or-later
// What the sequence tabs, the aspect switcher, their menus, the keys and the palette do
// (WP-2B.03). Every change of the document is one catalog command, so MCP and the CLI have the
// same call and each is one undo step: `sequence.create`, `sequence.duplicate`,
// `sequence.rename`, `sequence.delete`, `sequence.set_format`. Showing another sequence and the
// tabs themselves are view state (`view.set`), never an undo step. The engine is the authority
// (the last sequence cannot be deleted, a name must not be blank): its message is shown as it is.
import { RpcError, type SequenceId } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import { type EngineStoreState, engineStore } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { preferences } from "../prefs/prefsStore.ts";
import { type DocStore, docStore } from "../state/docStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { type ToastStore, toastStore } from "../state/toastStore.ts";
import { type ViewStore, viewStore } from "../state/viewStore.ts";
import { type Translate, translator } from "../timeline/errors.ts";
import { type AspectPreset, presetById } from "./presets.ts";
import {
  closed,
  closedOthers,
  freshName,
  moved,
  neighbour,
  nudged,
  placedAfter,
  reopened,
  type SequenceTab,
  sequenceListKey,
  sequencesOfKey,
} from "./tabs.ts";
import { sequenceUiStore } from "./uiStore.ts";

export interface SequenceDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly doc: StoreApi<DocStore>;
  readonly view: StoreApi<ViewStore>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  /** Settings > Timeline > Main track is magnetic in new sequences (ADR-0037). */
  readonly magneticMain: () => boolean;
}

/** The app's own stores. */
export function sequenceDeps(): SequenceDeps {
  return {
    engine: engineStore,
    doc: docStore,
    view: viewStore,
    toasts: toastStore,
    t: translator(i18n.t.bind(i18n)),
    magneticMain: () => preferences.get("timeline.magnetic_main"),
  };
}

const messageOf = (error: unknown): string =>
  error instanceof RpcError ? error.message : String(error);

function fail(deps: SequenceDeps, title: string, error: unknown): void {
  deps.toasts.getState().push({ tone: "danger", title, message: messageOf(error) });
}

const sequences = (deps: SequenceDeps): readonly SequenceTab[] =>
  sequencesOfKey(sequenceListKey(deps.doc.getState().project));

/** The sequence the editor shows now. */
export function shownSequence(deps: SequenceDeps = sequenceDeps()): SequenceId | null {
  return selectActiveSequenceId(deps.doc.getState().project);
}

/** Shows `sequence` in the timeline and the preview; a closed one gets its tab back. */
export function showSequence(sequence: SequenceId, deps: SequenceDeps = sequenceDeps()): void {
  const view = deps.view.getState();
  const tabs = reopened(view.sequenceTabs, sequence);
  if (tabs !== view.sequenceTabs) view.setSequenceTabs(tabs);
  view.setActiveSequence(sequence);
}

/** Shows the next (`1`) or the previous (`-1`) open tab, wrapping around. */
export function showNeighbour(by: -1 | 1, deps: SequenceDeps = sequenceDeps()): void {
  const next = neighbour(
    sequences(deps),
    deps.view.getState().sequenceTabs,
    shownSequence(deps),
    by,
  );
  if (next !== null) showSequence(next, deps);
}

/**
 * Adds a sequence and shows it: `sequence.create` with the first free "Sequence N" name, the
 * aspect preset when one is given (else the project's format of new sequences), and V1 magnetic
 * as Settings > Timeline says (ADR-0037). Resolves to its ID, or `null` after a toast.
 */
export async function createSequence(
  preset: AspectPreset["id"] | null = null,
  deps: SequenceDeps = sequenceDeps(),
): Promise<SequenceId | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  const name = freshName(sequences(deps), (n) => deps.t("sequences.newName", { n: String(n) }));
  try {
    const out = await rpc.call("sequence.create", {
      name,
      ...(preset !== null && { preset }),
      main_magnetic: deps.magneticMain(),
    });
    showSequence(out.sequence, deps);
    return out.sequence;
  } catch (error) {
    fail(deps, deps.t("sequences.failed.create"), error);
    return null;
  }
}

/** Copies `sequence` (`sequence.duplicate`), puts the copy's tab next to it and shows the copy. */
export async function duplicateSequence(
  sequence: SequenceId,
  deps: SequenceDeps = sequenceDeps(),
): Promise<SequenceId | null> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return null;
  try {
    const out = await rpc.call("sequence.duplicate", { sequence });
    const view = deps.view.getState();
    const all = [...sequences(deps).filter((s) => s.id !== out.sequence)];
    all.push({ id: out.sequence, name: out.name });
    view.setSequenceTabs(placedAfter(all, view.sequenceTabs, out.sequence, sequence));
    view.setActiveSequence(out.sequence);
    return out.sequence;
  } catch (error) {
    fail(deps, deps.t("sequences.failed.duplicate"), error);
    return null;
  }
}

/** Renames `sequence`. A blank or unchanged name is not a command. Resolves to success. */
export async function renameSequence(
  sequence: SequenceId,
  name: string,
  deps: SequenceDeps = sequenceDeps(),
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  const next = name.trim();
  const current = sequences(deps).find((s) => s.id === sequence)?.name;
  if (rpc === null || next === "" || next === current) return false;
  try {
    await rpc.call("sequence.rename", { sequence, name: next });
    return true;
  } catch (error) {
    fail(deps, deps.t("sequences.failed.rename"), error);
    return false;
  }
}

/** Deletes `sequence` with its clips (`sequence.delete`): one undo step brings it back. */
export async function deleteSequence(
  sequence: SequenceId,
  deps: SequenceDeps = sequenceDeps(),
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  // What shows next is decided before the delete, while the tab still has neighbours.
  const view = deps.view.getState();
  const next = closed(sequences(deps), view.sequenceTabs, shownSequence(deps), sequence);
  try {
    await rpc.call("sequence.delete", { sequence });
    if (next?.show != null) showSequence(next.show, deps);
    return true;
  } catch (error) {
    fail(deps, deps.t("sequences.failed.delete"), error);
    return false;
  }
}

/**
 * Gives `sequence` the canvas of an aspect preset (`sequence.set_format`; the clips follow as
 * its `reframe` says, `fit` unless told otherwise). The preset it already has is not a command.
 */
export async function setSequenceAspect(
  sequence: SequenceId,
  preset: AspectPreset["id"],
  deps: SequenceDeps = sequenceDeps(),
): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  const shape = presetById(preset);
  const format = deps.doc.getState().project?.sequences[sequence]?.format;
  if (rpc === null || shape === null || format === undefined) return false;
  if (format.width === shape.width && format.height === shape.height) return false;
  try {
    await rpc.call("sequence.set_format", {
      sequence,
      width: shape.width,
      height: shape.height,
      reframe: "fit",
    });
    return true;
  } catch (error) {
    fail(deps, deps.t("sequences.failed.aspect"), error);
    return false;
  }
}

/** Closes the tab of `sequence` (the sequence stays in the project). The last tab stays. */
export function closeTab(sequence: SequenceId, deps: SequenceDeps = sequenceDeps()): boolean {
  const view = deps.view.getState();
  const next = closed(sequences(deps), view.sequenceTabs, shownSequence(deps), sequence);
  if (next === null) return false;
  // The neighbour shows first: the active sequence always has a tab.
  if (next.show !== null) view.setActiveSequence(next.show);
  view.setSequenceTabs(next.tabs);
  return true;
}

/** Closes every tab but the one of `sequence`, and shows it. */
export function closeOtherTabs(sequence: SequenceId, deps: SequenceDeps = sequenceDeps()): void {
  const view = deps.view.getState();
  view.setActiveSequence(sequence);
  view.setSequenceTabs(closedOthers(sequences(deps), view.sequenceTabs, sequence));
}

/** Moves the tab of `sequence` to where the tab of `before` is (`null`: the end). */
export function moveTab(
  sequence: SequenceId,
  before: SequenceId | null,
  deps: SequenceDeps = sequenceDeps(),
): void {
  const view = deps.view.getState();
  const next = moved(sequences(deps), view.sequenceTabs, sequence, before);
  if (next !== view.sequenceTabs) view.setSequenceTabs(next);
}

/** Moves the tab of `sequence` one place left or right. */
export function nudgeTab(
  sequence: SequenceId,
  by: -1 | 1,
  deps: SequenceDeps = sequenceDeps(),
): void {
  const view = deps.view.getState();
  const next = nudged(sequences(deps), view.sequenceTabs, shownSequence(deps), sequence, by);
  if (next !== view.sequenceTabs) view.setSequenceTabs(next);
}

/** Starts renaming `sequence` where its tab shows its name. */
export function beginRename(sequence: SequenceId, deps: SequenceDeps = sequenceDeps()): void {
  showSequence(sequence, deps);
  sequenceUiStore.getState().startRename(sequence);
}

/** Opens Sequence settings for `sequence` (default: the one that shows). */
export function openSequenceSettings(
  sequence: SequenceId | null = null,
  deps: SequenceDeps = sequenceDeps(),
): void {
  const target = sequence ?? shownSequence(deps);
  if (target !== null) sequenceUiStore.getState().openSettings(target);
}
