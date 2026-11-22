// SPDX-License-Identifier: GPL-3.0-or-later
// Resolves design tokens from the CSS variables in packages/ui/src/tokens/tokens.css.
// No literal colors or sizes in this spike: everything comes from getComputedStyle.
import type { Kind } from "./model.ts";

export interface Theme {
  surface0: string;
  surface1: string;
  surface3: string;
  lane: string;
  laneAlt: string;
  textPrimary: string;
  textMuted: string;
  playhead: string;
  marigold: string;
  marigoldSubtle: string;
  clip: Record<Kind, { edge: string; body: string }>;
  fontUi: string;
  fontMono: string;
  stripPx: number;
  trackSm: number;
  trackMd: number;
  trackLg: number;
  space1: number;
  space2: number;
}

export function readTheme(): Theme {
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string): string => {
    const s = cs.getPropertyValue(name).trim();
    if (!s) throw new Error(`missing design token ${name}`);
    return s;
  };
  const px = (name: string): number => Number.parseFloat(v(name));
  const kind = (k: Kind) => ({ edge: v(`--clip-${k}`), body: v(`--clip-${k}-body`) });
  return {
    surface0: v("--surface-0"),
    surface1: v("--surface-1"),
    surface3: v("--surface-3"),
    lane: v("--timeline-lane"),
    laneAlt: v("--timeline-lane-alt"),
    textPrimary: v("--text-primary"),
    textMuted: v("--text-muted"),
    playhead: v("--playhead"),
    marigold: v("--marigold"),
    marigoldSubtle: v("--marigold-subtle"),
    clip: {
      media: kind("media"),
      audio: kind("audio"),
      graphics: kind("graphics"),
      captions: kind("captions"),
      effects: kind("effects"),
    },
    fontUi: v("--font-ui"),
    fontMono: v("--font-mono"),
    stripPx: px("--clip-strip"),
    trackSm: px("--track-sm"),
    trackMd: px("--track-md"),
    trackLg: px("--track-lg"),
    space1: px("--space-1"),
    space2: px("--space-2"),
  };
}
