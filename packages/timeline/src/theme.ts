// SPDX-License-Identifier: GPL-3.0-or-later
// Resolves design tokens (CSS variables from packages/ui/src/tokens/tokens.css) into a plain
// object once per theme change, so the draw loop never touches the DOM or hard-codes a color.
import type { ItemKind } from "./model.ts";

export interface ClipColors {
  /** `clip-<type>`: border, header strip, waveform. */
  readonly strong: string;
  /** `clip-<type>-body`. */
  readonly body: string;
}

export interface Theme {
  readonly surface0: string;
  readonly surface1: string;
  readonly surface3: string;
  readonly lane: string;
  readonly laneAlt: string;
  readonly borderSubtle: string;
  readonly textPrimary: string;
  readonly textSecondary: string;
  readonly textMuted: string;
  readonly playhead: string;
  readonly onMarigold: string;
  readonly marigold: string;
  readonly marigoldSubtle: string;
  readonly marigoldText: string;
  readonly agent: string;
  readonly agentSubtle: string;
  readonly clip: Readonly<Record<ItemKind, ClipColors>>;
  /** Canvas `font` shorthands built from the `caption`, `label` and `numeric` type styles. */
  readonly fontCaption: string;
  readonly fontLabel: string;
  readonly fontNumeric: string;
  readonly captionPx: number;
  readonly numericPx: number;
  readonly opacityDisabled: number;
  readonly borderThin: number;
  readonly clipStrip: number;
  readonly radiusSm: number;
  readonly controlSm: number;
  readonly controlMd: number;
  readonly iconSm: number;
  readonly trackSm: number;
  readonly trackMd: number;
  readonly trackLg: number;
  readonly space0_5: number;
  readonly space1: number;
  readonly space1_5: number;
  readonly space2: number;
  readonly space4: number;
  readonly space12: number;
}

const KINDS: readonly ItemKind[] = ["media", "audio", "graphics", "captions", "effects"];

/**
 * Reads every token from `root`'s computed style (default `document.documentElement`). Throws on
 * a missing token so a broken theme fails loudly instead of drawing transparent pixels.
 */
export function readTheme(root: HTMLElement = document.documentElement): Theme {
  const cs = getComputedStyle(root);
  const v = (name: string): string => {
    const s = cs.getPropertyValue(name).trim();
    if (!s) throw new Error(`@cutweave/timeline: missing design token ${name}`);
    return s;
  };
  const px = (name: string): number => Number.parseFloat(v(name));
  const clip = (k: ItemKind): ClipColors => ({
    strong: v(`--clip-${k}`),
    body: v(`--clip-${k}-body`),
  });

  // Type styles are composite tokens (generated `.caption`, `.label`, `.numeric` classes), which a
  // canvas cannot read directly; probe each class once and build the `font` shorthand from it.
  const probe = (cls: string): { font: string; size: number } => {
    const el = document.createElement("span");
    el.className = cls;
    el.style.position = "absolute";
    el.style.visibility = "hidden";
    root.appendChild(el);
    const s = getComputedStyle(el);
    const size = Number.parseFloat(s.fontSize);
    const font = `${s.fontWeight} ${s.fontSize} ${s.fontFamily}`;
    el.remove();
    if (!Number.isFinite(size)) throw new Error(`@cutweave/timeline: type style .${cls} missing`);
    return { font, size };
  };
  const caption = probe("caption");
  const label = probe("label");
  const numeric = probe("numeric");

  return {
    surface0: v("--surface-0"),
    surface1: v("--surface-1"),
    surface3: v("--surface-3"),
    lane: v("--timeline-lane"),
    laneAlt: v("--timeline-lane-alt"),
    borderSubtle: v("--border-subtle"),
    textPrimary: v("--text-primary"),
    textSecondary: v("--text-secondary"),
    textMuted: v("--text-muted"),
    playhead: v("--playhead"),
    onMarigold: v("--on-marigold"),
    marigold: v("--marigold"),
    marigoldSubtle: v("--marigold-subtle"),
    marigoldText: v("--marigold-text"),
    agent: v("--agent"),
    agentSubtle: v("--agent-subtle"),
    clip: Object.fromEntries(KINDS.map((k) => [k, clip(k)])) as Record<ItemKind, ClipColors>,
    fontCaption: caption.font,
    fontLabel: label.font,
    fontNumeric: numeric.font,
    captionPx: caption.size,
    numericPx: numeric.size,
    opacityDisabled: Number.parseFloat(v("--opacity-disabled")),
    borderThin: px("--border-thin"),
    clipStrip: px("--clip-strip"),
    radiusSm: px("--radius-sm"),
    controlSm: px("--control-sm"),
    controlMd: px("--control-md"),
    iconSm: px("--icon-sm"),
    trackSm: px("--track-sm"),
    trackMd: px("--track-md"),
    trackLg: px("--track-lg"),
    space0_5: px("--space-0-5"),
    space1: px("--space-1"),
    space1_5: px("--space-1-5"),
    space2: px("--space-2"),
    space4: px("--space-4"),
    space12: px("--space-12"),
  };
}
