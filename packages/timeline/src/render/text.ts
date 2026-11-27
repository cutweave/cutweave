// SPDX-License-Identifier: GPL-3.0-or-later
// Strings the draw loop shows every frame, made once and reused: a frame that only pans or plays
// asks for labels it already made, so it allocates no strings (WP-2A.09: no per-frame allocation).
import type { TimeFormat } from "../model.ts";
import {
  type FrameRate,
  flicksToFrame,
  formatFrameTimecode,
  formatSeconds,
  frameToFlicks,
} from "../time.ts";

/** Most time labels kept; past it the cache starts over (a long pan at a fine zoom). */
const MAX_TIME_LABELS = 2048;

/** Ruler and playhead-flag labels by frame, for one frame rate, drop-frame flag and format. */
export class TimeLabels {
  private readonly byFrame = new Map<number, string>();
  private num = 0;
  private den = 0;
  private drop = false;
  private format: TimeFormat = "timecode";

  /** The label of the frame that contains time `t` (flicks). */
  label(t: number, fps: FrameRate, dropFrame: boolean, format: TimeFormat): string {
    if (
      fps.num !== this.num ||
      fps.den !== this.den ||
      dropFrame !== this.drop ||
      format !== this.format
    ) {
      this.byFrame.clear();
      this.num = fps.num;
      this.den = fps.den;
      this.drop = dropFrame;
      this.format = format;
    }
    const frame = Math.max(0, flicksToFrame(t, fps));
    let s = this.byFrame.get(frame);
    if (s === undefined) {
      if (this.byFrame.size >= MAX_TIME_LABELS) this.byFrame.clear();
      s =
        format === "frames"
          ? String(frame)
          : format === "seconds"
            ? formatSeconds(frameToFlicks(frame, fps))
            : formatFrameTimecode(frame, fps, dropFrame);
      this.byFrame.set(frame, s);
    }
    return s;
  }
}

/**
 * Labels cut to a width with an ellipsis, per entry (item or track index) of one owner (a
 * layout): the cut string is kept until the room for it changes, so only zooming makes new ones.
 */
export class FittedLabels {
  private owner: object | null = null;
  private chars = new Int32Array(0);
  private cut: string[] = [];

  /** `text` cut to `maxPx` at `charPx` per character ("" when not even two characters fit). */
  fit(owner: object, size: number, i: number, text: string, maxPx: number, charPx: number): string {
    const maxChars = Math.floor(maxPx / charPx);
    if (text.length <= maxChars) return text;
    if (maxChars <= 1) return "";
    if (owner !== this.owner || this.chars.length !== size) {
      this.owner = owner;
      this.chars = new Int32Array(size).fill(-1);
      this.cut = new Array<string>(size).fill("");
    }
    if (this.chars[i] === maxChars) return this.cut[i] as string;
    const s = `${text.slice(0, maxChars - 1)}…`;
    this.chars[i] = maxChars;
    this.cut[i] = s;
    return s;
  }
}
