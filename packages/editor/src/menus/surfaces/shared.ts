// SPDX-License-Identifier: GPL-3.0-or-later
// Small pieces the surface tables share (WP-2H.13): the reveal label for this platform, the
// reasons a row gives, and the frame-rate lookups of the timecode copies.
import type { FrameRate } from "@cutweave/timeline";
import type { TKey } from "../../i18n/index.ts";
import { menuPlatform } from "../../shell/appMenu.ts";
import { docStore } from "../../state/docStore.ts";
import { selectActiveSequenceId } from "../../state/selectors.ts";
import type { LabelSpec, MenuContext, MenuRow } from "../types.ts";

/** "Reveal in Explorer", "Reveal in Finder" or "Reveal in file manager". */
export function revealLabel(): LabelSpec {
  const platform = menuPlatform(typeof navigator === "undefined" ? "" : navigator.platform);
  if (platform === "windows") return { key: "contextMenu.item.revealExplorer" };
  if (platform === "mac") return { key: "contextMenu.item.revealFinder" };
  return { key: "contextMenu.item.revealFiles" };
}

/** The rate the active sequence runs at, for a timecode. */
export function sequenceRate(): FrameRate | null {
  const project = docStore.getState().project;
  const id = selectActiveSequenceId(project);
  const rate = id === null ? undefined : project?.sequences[id]?.format.frame_rate;
  return rate === undefined ? null : { num: rate.num, den: rate.den };
}

export const NEEDS_ENGINE: TKey = "contextMenu.reason.noEngine";

/** A row that needs the engine to be running (UI commands are not checked like catalog ones). */
export function needsEngine(row: MenuRow): MenuRow {
  return { ...row, when: "engine.live", reason: NEEDS_ENGINE };
}

/** The data a surface resolver puts on the target. */
export function dataOf<T>(ctx: MenuContext): T {
  return ctx.target.data as T;
}
