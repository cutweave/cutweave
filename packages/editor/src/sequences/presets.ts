// SPDX-License-Identifier: GPL-3.0-or-later
// The aspect presets the editor offers (WP-2B.03): the engine's own table
// (`crates/cutweave-model/src/presets.rs`, WP-2B.01; `sequence.list{presets: true}` returns it),
// in the same order and with the same IDs, so `sequence.create{preset}` and a
// `sequence.set_format` with a preset's size mean the same canvas.
import type { SequenceFormat } from "@cutweave/protocol";

export interface AspectPreset {
  /** The engine's preset ID. */
  readonly id: "16:9" | "16:9-4k" | "9:16" | "1:1" | "4:5" | "21:9";
  /** The shape as people say it: `16:9`. */
  readonly ratio: string;
  readonly width: number;
  readonly height: number;
}

export const ASPECT_PRESETS: readonly AspectPreset[] = [
  { id: "16:9", ratio: "16:9", width: 1920, height: 1080 },
  { id: "16:9-4k", ratio: "16:9", width: 3840, height: 2160 },
  { id: "9:16", ratio: "9:16", width: 1080, height: 1920 },
  { id: "1:1", ratio: "1:1", width: 1080, height: 1080 },
  { id: "4:5", ratio: "4:5", width: 1080, height: 1350 },
  { id: "21:9", ratio: "21:9", width: 2560, height: 1080 },
];

export function presetById(id: string): AspectPreset | null {
  return ASPECT_PRESETS.find((p) => p.id === id) ?? null;
}

/** The preset whose canvas `format` has exactly; `null` for a custom size. */
export function presetOfFormat(
  format: Pick<SequenceFormat, "width" | "height"> | undefined,
): AspectPreset | null {
  if (format === undefined) return null;
  return ASPECT_PRESETS.find((p) => p.width === format.width && p.height === format.height) ?? null;
}
