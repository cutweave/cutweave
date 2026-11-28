// SPDX-License-Identifier: GPL-3.0-or-later
// The sequence tab's menu (`sequence.tab`, WP-2B.03; plan in docs/research/09-context-menus.md):
// Rename, Duplicate, Sequence settings…, Aspect ▸ (the presets), Move left and right, Close,
// Close others and Delete sequence. The same commands, acting on the sequence that shows, are in
// the command palette, and the ones with a key are in the menu bar.
//
// A document change is one catalog command sent by `sequences/actions.ts`
// (`sequence.duplicate`, `sequence.rename`, `sequence.set_format`, `sequence.delete`,
// `sequence.create`), so each is one undo step and MCP and the CLI have the same call. Showing a
// sequence and the tabs are view state.
import type { SequenceId } from "@cutweave/protocol";
import {
  beginRename,
  closeOtherTabs,
  closeTab,
  createSequence,
  deleteSequence,
  duplicateSequence,
  nudgeTab,
  openSequenceSettings,
  setSequenceAspect,
  showNeighbour,
  shownSequence,
} from "../../sequences/actions.ts";
import { ASPECT_PRESETS, presetOfFormat } from "../../sequences/presets.ts";
import { openTabs, sequenceListKey, sequencesOfKey } from "../../sequences/tabs.ts";
import { docStore } from "../../state/docStore.ts";
import { viewStore } from "../../state/viewStore.ts";
import type { RunContext, UiCommand } from "../commands.ts";
import type { MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";

/** The sequence a choice acts on: the tab the menu is on, else the one that shows. */
function targetOf(args: unknown, ctx: Pick<RunContext, "ids">): SequenceId | null {
  const named = (args as { sequence?: string } | null | undefined)?.sequence ?? ctx.ids[0];
  const project = docStore.getState().project;
  if (named !== undefined && project?.sequences[named as SequenceId] !== undefined) {
    return named as SequenceId;
  }
  return shownSequence();
}

const all = () => sequencesOfKey(sequenceListKey(docStore.getState().project));
const open = () => openTabs(all(), viewStore.getState().sequenceTabs, shownSequence());
const severalOpen = (): boolean => open().length > 1;
const severalSequences = (): boolean => all().length > 1;
const tabArgs = (ctx: MenuContext) => ({ sequence: ctx.ids[0] });

/** Whether the tab of the menu's sequence has a neighbour on that side. */
function hasNeighbour(ctx: MenuContext, by: -1 | 1): boolean {
  const tabs = open();
  const at = tabs.findIndex((s) => s.id === ctx.ids[0]);
  return at >= 0 && tabs[at + by] !== undefined;
}

export const SEQUENCE_UI_COMMANDS: readonly UiCommand[] = [
  {
    // "+" above the timeline, File > New sequence and the palette: `sequence.create` with the
    // project's format of new sequences, then the new sequence shows.
    id: "sequences.new",
    label: "sequences.new",
    icon: "plus",
    when: "engine.live",
    run: async () => void (await createSequence()),
  },
  {
    id: "sequences.rename",
    label: "sequences.rename",
    when: "engine.live",
    run: (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) beginRename(sequence);
    },
  },
  {
    id: "sequences.duplicate",
    label: "sequences.duplicate",
    icon: "copy",
    when: "engine.live",
    run: async (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) await duplicateSequence(sequence);
    },
  },
  {
    id: "sequences.settings",
    label: "sequences.settings",
    icon: "sliders-horizontal",
    when: "engine.live",
    run: (args, ctx) => openSequenceSettings(targetOf(args, ctx)),
  },
  {
    // Aspect ▸ and the preview's switcher: one `sequence.set_format` with the preset's canvas.
    id: "sequences.set_aspect",
    label: "sequences.aspect",
    hidden: true,
    run: async (args, ctx) => {
      const sequence = targetOf(args, ctx);
      const preset = (args as { preset?: string } | null | undefined)?.preset;
      const known = ASPECT_PRESETS.find((p) => p.id === preset);
      if (sequence !== null && known !== undefined) await setSequenceAspect(sequence, known.id);
    },
  },
  {
    id: "sequences.move_left",
    label: "sequences.moveLeft",
    icon: "arrow-left",
    hidden: true,
    run: (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) nudgeTab(sequence, -1);
    },
  },
  {
    id: "sequences.move_right",
    label: "sequences.moveRight",
    icon: "arrow-right",
    hidden: true,
    run: (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) nudgeTab(sequence, 1);
    },
  },
  {
    id: "sequences.close",
    label: "sequences.close",
    icon: "x",
    run: (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) closeTab(sequence);
    },
  },
  {
    id: "sequences.close_others",
    label: "sequences.closeOthers",
    run: (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) closeOtherTabs(sequence);
    },
  },
  {
    id: "sequences.next",
    label: "sequences.next",
    run: () => showNeighbour(1),
  },
  {
    id: "sequences.previous",
    label: "sequences.previous",
    run: () => showNeighbour(-1),
  },
  {
    id: "sequences.delete",
    label: "sequences.delete",
    icon: "trash-2",
    danger: true,
    when: "engine.live",
    run: async (args, ctx) => {
      const sequence = targetOf(args, ctx);
      if (sequence !== null) await deleteSequence(sequence);
    },
  },
];

/** The sequence commands the palette lists: they act on the sequence that shows. */
export const SEQUENCE_PALETTE_COMMANDS: readonly string[] = [
  "sequences.new",
  "sequences.rename",
  "sequences.duplicate",
  "sequences.settings",
  "sequences.close",
  "sequences.close_others",
  "sequences.next",
  "sequences.previous",
  "sequences.delete",
];

const aspectRows = (): readonly MenuRow[] =>
  ASPECT_PRESETS.map(
    (preset, order): MenuRow => ({
      id: `aspect-${preset.id}`,
      command: "sequences.set_aspect",
      group: "format",
      order,
      label: {
        key: "sequences.presetSize",
        vars: { ratio: preset.ratio, width: preset.width, height: preset.height },
      },
      radio: true,
      checked: (ctx) =>
        presetOfFormat(docStore.getState().project?.sequences[ctx.ids[0] as SequenceId]?.format)
          ?.id === preset.id,
      args: (ctx) => ({ sequence: ctx.ids[0], preset: preset.id }),
    }),
  );

const TAB_ROWS: readonly MenuRow[] = [
  { command: "sequences.rename", group: "edit", order: 0, args: tabArgs },
  { command: "sequences.duplicate", group: "edit", order: 1, args: tabArgs },
  { command: "sequences.settings", group: "format", order: 0, args: tabArgs },
  {
    id: "aspect",
    group: "format",
    order: 1,
    submenu: { label: { key: "sequences.aspect" }, rows: aspectRows() },
  },
  {
    command: "sequences.move_left",
    group: "tabs",
    order: 0,
    args: tabArgs,
    enabledIf: (ctx) => hasNeighbour(ctx, -1),
    reason: "sequences.reason.firstTab",
  },
  {
    command: "sequences.move_right",
    group: "tabs",
    order: 1,
    args: tabArgs,
    enabledIf: (ctx) => hasNeighbour(ctx, 1),
    reason: "sequences.reason.lastInRow",
  },
  {
    command: "sequences.close",
    group: "tabs",
    order: 2,
    args: tabArgs,
    enabledIf: severalOpen,
    reason: "sequences.reason.lastTab",
  },
  {
    command: "sequences.close_others",
    group: "tabs",
    order: 3,
    args: tabArgs,
    enabledIf: severalOpen,
    reason: "sequences.reason.lastTab",
  },
  {
    command: "sequences.delete",
    group: "delete",
    order: 0,
    args: tabArgs,
    variant: "danger",
    enabledIf: severalSequences,
    reason: "sequences.reason.lastSequence",
  },
];

export const SEQUENCE_TAB: MenuSurfaceDef = {
  id: "sequence.tab",
  name: (target) => ({ key: "contextMenu.name.sequenceTab", vars: { name: target.name ?? "" } }),
  groups: ["edit", "format", "tabs", "delete"],
  rows: TAB_ROWS,
};
