// SPDX-License-Identifier: GPL-3.0-or-later
// What the transport bar shows at each width (#225). The bar lives in a panel that can be any
// width (docked, floating or in a window of its own), so it never looks at the window: it
// measures what its controls need and moves the least important ones into its "More" menu until
// the rest fit, and brings them back as soon as there is room.

/** The controls that can move into the More menu. Play/pause and the timecode never do. */
export type TransportControl =
  | "fullscreen"
  | "source"
  | "quality"
  | "proxies"
  | "volume"
  | "loop"
  | "mute"
  | "toStart"
  | "toEnd"
  | "stepBack"
  | "stepForward";

/**
 * The order controls leave the bar in, least important first. Level `n` has moved the first `n`
 * steps into the More menu:
 *
 * 1. full screen and the Proxy/Original chip (a button that does nothing yet and a readout);
 * 2. the quality readout and the proxies toggle (set once, both also in Settings > Playback);
 * 3. the volume slider (Mute stays);
 * 4. loop;
 * 5. mute, go to start and go to end;
 * 6. the frame steps: only for a window narrower than the docked panel can be. What is left is
 *    play/pause, the timecode and More.
 *
 * A step has to free more room than the More button takes, which is why full screen (as wide as
 * More) does not go alone.
 */
export const COLLAPSE_STEPS: readonly (readonly TransportControl[])[] = [
  ["fullscreen", "source"],
  ["quality", "proxies"],
  ["volume"],
  ["loop"],
  ["mute", "toStart", "toEnd"],
  ["stepBack", "stepForward"],
];

/** The level with everything that can move in the More menu. */
export const MAX_LEVEL = COLLAPSE_STEPS.length;

/** The controls that are in the More menu at `level` (0: none). */
export function collapsedAt(level: number): ReadonlySet<TransportControl> {
  return new Set(COLLAPSE_STEPS.slice(0, Math.max(0, level)).flat());
}

/** Widths within this many CSS pixels are the same width (sub-pixel layout). */
const SLACK = 0.5;

/**
 * The level to show in `available` CSS pixels. `needs[n]` is the width level `n` took when it was
 * last on screen, or `undefined` if it has not been measured: the answer is the first level that
 * fits or that has to be tried to find out. The bar shows it, measures it and asks again, so it
 * settles on the first level that fits (the last one when nothing does).
 */
export function fitLevel(available: number, needs: readonly (number | undefined)[]): number {
  for (let level = 0; level < MAX_LEVEL; level += 1) {
    const need = needs[level];
    if (need === undefined || need <= available + SLACK) return level;
  }
  return MAX_LEVEL;
}

/**
 * Records that `level` needs `need` pixels. When that is not what it needed before, the content
 * itself changed size (density, language, the time display), so what the other levels needed is
 * forgotten and they are measured again.
 */
export function recordNeed(
  needs: readonly (number | undefined)[],
  level: number,
  need: number,
): (number | undefined)[] {
  const before = needs[level];
  const next: (number | undefined)[] =
    before !== undefined && Math.abs(before - need) > SLACK ? [] : [...needs];
  next[level] = need;
  return next;
}
