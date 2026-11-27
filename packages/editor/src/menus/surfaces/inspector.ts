// SPDX-License-Identifier: GPL-3.0-or-later
// The Inspector's menus and the Settings row's (WP-2H.13): a section, a property row, the media
// path, the media picture, a setting.
// TODO(WP-2C.03 follow-up): Paste value.
// TODO(render.frame): the picture's Copy frame and Save frame as….
import type { MediaId, PropId, ResetGroup } from "@cutweave/protocol";
import type { MenuContext, MenuSurfaceDef } from "../types.ts";
import { playableMedia } from "./bin.ts";
import { clipFact, keyAtPlayhead, keysOf, mediaPath } from "./facts.ts";
import { revealLabel } from "./shared.ts";

/**
 * What a section header says about itself when it opens its menu. `ids` of the target are the
 * clips the section edits; `group` is the engine's reset group of the section (a section with
 * none, like Timing, cannot be reset).
 */
export interface SectionData {
  readonly group?: ResetGroup;
}

const LOCKED = "contextMenu.reason.trackLocked" as const;

/** Whether every clip the menu acts on sits on an unlocked track. */
const unlocked = (ctx: MenuContext): boolean =>
  ctx.ids.length > 0 && ctx.ids.every((id) => clipFact(id)?.track.locked === false);

export const INSPECTOR_SECTION: MenuSurfaceDef = {
  id: "inspector.section",
  name: (target) => ({ key: "contextMenu.name.section", vars: { name: target.name ?? "" } }),
  groups: ["reset", "sections"],
  rows: [
    {
      id: "reset-section",
      command: "item.reset_props",
      group: "reset",
      order: 0,
      label: { key: "contextMenu.item.resetSection" },
      enabledIf: (ctx) =>
        (ctx.target.data as SectionData | undefined)?.group !== undefined && unlocked(ctx),
      reason: (ctx) =>
        ctx.ids.length > 0 && !unlocked(ctx) ? LOCKED : "contextMenu.reason.noReset",
      args: (ctx) => ({
        items: ctx.ids,
        group: (ctx.target.data as SectionData | undefined)?.group,
      }),
    },
    { command: "inspector.collapse_all", group: "sections", order: 0 },
    { command: "inspector.expand_all", group: "sections", order: 1 },
  ],
};

/**
 * What a property row says about itself when it opens its menu. `prop` is the engine's ID of
 * the property the row edits (absent for a row that is not animatable, like Name); the clips are
 * the target's `ids`.
 */
export interface RowData {
  readonly text: string;
  readonly prop?: PropId;
}

const rowProp = (ctx: MenuContext): PropId | undefined => (ctx.target.data as RowData).prop;

/** The one clip a keyframe row acts on, its property and the key the playhead sits on, if any. */
function keyState(ctx: MenuContext) {
  const prop = rowProp(ctx);
  const id = ctx.ids.length === 1 ? ctx.ids[0] : undefined;
  const fact = id === undefined ? null : clipFact(id);
  if (prop === undefined || fact === null || ctx.playhead === null) return null;
  const { clip } = fact;
  return {
    prop,
    locked: fact.track.locked,
    inside: ctx.playhead >= clip.start && ctx.playhead < clip.start + clip.duration,
    animated: keysOf(clip, prop) !== null,
    key: keyAtPlayhead(clip, prop, ctx.playhead),
  };
}

/** Why Add or Remove keyframe cannot run, in order of what the person can fix first. */
const keyReason = (ctx: MenuContext) => {
  const s = keyState(ctx);
  if (ctx.ids.length !== 1) return "contextMenu.reason.oneClip" as const;
  if (s === null) return "contextMenu.reason.noKeyframes" as const;
  if (s.locked) return LOCKED;
  if (s.key !== null) return "contextMenu.reason.keyframeHere" as const;
  if (!s.inside) return "contextMenu.reason.playheadOutside" as const;
  return s.animated
    ? ("contextMenu.reason.noKeyframeHere" as const)
    : ("contextMenu.reason.noKeyframes" as const);
};

export const INSPECTOR_ROW: MenuSurfaceDef = {
  id: "inspector.row",
  name: (target) => ({ key: "contextMenu.name.property", vars: { name: target.name ?? "" } }),
  groups: ["keyframe", "reset", "copy"],
  rows: [
    {
      id: "add-keyframe",
      command: "keyframe.set",
      group: "keyframe",
      order: 0,
      label: { key: "contextMenu.item.addKeyframe" },
      enabledIf: (ctx) => {
        const s = keyState(ctx);
        return s !== null && !s.locked && s.inside && s.key === null;
      },
      reason: keyReason,
      args: (ctx) => ({
        item: ctx.ids[0],
        prop: rowProp(ctx),
        keys: [{ at: ctx.playhead }],
      }),
    },
    {
      id: "remove-keyframe",
      command: "keyframe.delete",
      group: "keyframe",
      order: 1,
      label: { key: "contextMenu.item.removeKeyframe" },
      enabledIf: (ctx) => {
        const s = keyState(ctx);
        return s !== null && !s.locked && s.key !== null;
      },
      reason: keyReason,
      args: (ctx) => {
        const s = keyState(ctx);
        return { item: ctx.ids[0], keys: [{ prop: s?.prop, t: s?.key }] };
      },
    },
    {
      // WP-2C.05: the curve editor for the key the playhead sits on.
      command: "timeline.edit_keyframe_curve",
      group: "keyframe",
      order: 2,
      enabledIf: (ctx) => {
        const s = keyState(ctx);
        return s !== null && !s.locked && s.key !== null;
      },
      reason: (ctx) => {
        const s = keyState(ctx);
        if (ctx.ids.length !== 1) return "contextMenu.reason.oneClip" as const;
        if (s?.locked === true) return LOCKED;
        return s?.animated === true
          ? ("contextMenu.reason.noKeyframeHere" as const)
          : ("contextMenu.reason.noKeyframes" as const);
      },
      args: (ctx) => {
        const s = keyState(ctx);
        return { item: ctx.ids[0], prop: s?.prop, t: s?.key ?? undefined };
      },
    },
    {
      id: "reset-row",
      command: "item.reset_props",
      group: "reset",
      order: 0,
      label: { key: "contextMenu.item.resetToDefault" },
      enabledIf: (ctx) => rowProp(ctx) !== undefined && unlocked(ctx),
      reason: (ctx) =>
        ctx.ids.length > 0 && !unlocked(ctx) ? LOCKED : "contextMenu.reason.noReset",
      args: (ctx) => ({ items: ctx.ids, props: [rowProp(ctx)] }),
    },
    {
      id: "copy-value",
      command: "host.copy_text",
      group: "copy",
      order: 0,
      label: { key: "contextMenu.item.copyValue" },
      args: (ctx: MenuContext) => ({ text: (ctx.target.data as RowData).text }),
    },
  ],
};

const hasPath = (ctx: MenuContext): boolean => mediaPath(ctx.ids[0] ?? "") !== null;

export const INSPECTOR_PATH: MenuSurfaceDef = {
  id: "inspector.path",
  name: () => ({ key: "contextMenu.name.mediaPath" }),
  groups: ["file"],
  rows: [
    {
      id: "copy-path",
      command: "host.copy_text",
      group: "file",
      order: 0,
      label: { key: "contextMenu.item.copyPath" },
      enabledIf: hasPath,
      reason: "contextMenu.reason.noFile",
      args: (ctx) => ({ text: mediaPath(ctx.ids[0] ?? "") ?? "" }),
    },
    {
      id: "reveal",
      command: "host.reveal_path",
      group: "file",
      order: 1,
      label: revealLabel,
      enabledIf: hasPath,
      reason: "contextMenu.reason.noFile",
      args: (ctx) => ({ path: mediaPath(ctx.ids[0] ?? "") }),
    },
  ],
};

export const INSPECTOR_PICTURE: MenuSurfaceDef = {
  id: "inspector.picture",
  name: () => ({ key: "contextMenu.name.mediaPicture" }),
  groups: ["play"],
  rows: [
    {
      command: "bin.play_in_source",
      group: "play",
      order: 0,
      enabledIf: (ctx) => playableMedia(ctx.ids[0] ?? ""),
      reason: "contextMenu.reason.notPlayable",
      args: (ctx) => ({ id: ctx.ids[0] as MediaId }),
    },
  ],
};

/** What a setting row says about itself. */
export interface SettingData {
  readonly key: string;
  readonly modified: boolean;
}

export const SETTINGS_ROW: MenuSurfaceDef = {
  id: "settings.row",
  name: (target) => ({ key: "contextMenu.name.setting", vars: { name: target.name ?? "" } }),
  groups: ["setting"],
  rows: [
    {
      command: "settings.reset_row",
      group: "setting",
      order: 0,
      enabledIf: (ctx) => (ctx.target.data as SettingData).modified,
      reason: "contextMenu.reason.alreadyDefault",
      args: (ctx) => ({ key: (ctx.target.data as SettingData).key }),
    },
    {
      id: "copy-key",
      command: "host.copy_text",
      group: "setting",
      order: 1,
      label: { key: "contextMenu.item.copySettingKey" },
      args: (ctx) => ({ text: (ctx.target.data as SettingData).key }),
    },
  ],
};
