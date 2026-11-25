// SPDX-License-Identifier: GPL-3.0-or-later
// The Inspector's menus and the Settings row's (WP-2H.13): a section, a property row, the media
// path, the media picture, a setting.
// TODO(WP-2C.03): Reset section, Reset to default, Add and Remove keyframe, Paste value.
// TODO(render.frame): the picture's Copy frame and Save frame as….
import type { MediaId } from "@cutweave/protocol";
import type { MenuContext, MenuSurfaceDef } from "../types.ts";
import { playableMedia } from "./bin.ts";
import { mediaPath } from "./facts.ts";
import { revealLabel } from "./shared.ts";

export const INSPECTOR_SECTION: MenuSurfaceDef = {
  id: "inspector.section",
  name: (target) => ({ key: "contextMenu.name.section", vars: { name: target.name ?? "" } }),
  groups: ["sections"],
  rows: [
    { command: "inspector.collapse_all", group: "sections", order: 0 },
    { command: "inspector.expand_all", group: "sections", order: 1 },
  ],
};

/** What a property row says about itself when it opens its menu. */
export interface RowData {
  readonly text: string;
}

export const INSPECTOR_ROW: MenuSurfaceDef = {
  id: "inspector.row",
  name: (target) => ({ key: "contextMenu.name.property", vars: { name: target.name ?? "" } }),
  groups: ["copy"],
  rows: [
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
