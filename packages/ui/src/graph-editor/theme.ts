// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's colours and sizes, read once per theme change from the design tokens (CSS
// variables), so the draw loop never touches the DOM and holds no literal colour or size.

export interface GraphTheme {
  readonly surface: string;
  readonly ruler: string;
  readonly grid: string;
  readonly gridStrong: string;
  readonly text: string;
  readonly textStrong: string;
  readonly playhead: string;
  /** Selected keys and their handles: marigold is "here". */
  readonly selected: string;
  readonly onSelected: string;
  readonly marquee: string;
  readonly guide: string;
  /** One colour per curve, in order; a curve's `color` indexes it (wrapping). */
  readonly curves: readonly string[];
  /** Canvas `font` shorthand of the `numeric` type style. */
  readonly font: string;
  readonly fontPx: number;
  readonly dimmed: number;
  readonly line: number;
  readonly lineStrong: number;
  /** Height of the time ruler and width of the value ruler. */
  readonly rulerHeight: number;
  readonly rulerWidth: number;
  /** A key's diamond, a handle's dot and the reach of a pointer around them. */
  readonly keySize: number;
  readonly handleSize: number;
  readonly hitRadius: number;
  readonly pad: number;
}

const CURVE_TOKENS = [
  "--label-blue",
  "--label-orange",
  "--label-green",
  "--label-pink",
  "--label-teal",
  "--label-purple",
  "--label-lime",
  "--label-red",
] as const;

/** How many curve colours the theme has. */
export const CURVE_COLORS = CURVE_TOKENS.length;

/** The token of curve colour `index` (wrapping), for a swatch beside a curve's name. */
export const curveColorToken = (index: number): string =>
  CURVE_TOKENS[((index % CURVE_COLORS) + CURVE_COLORS) % CURVE_COLORS] as string;

/**
 * Reads the tokens from `el`'s computed style (its own window's, so a detached panel reads its
 * own stylesheets). Throws on a missing token: a broken theme fails loudly.
 */
export function readGraphTheme(el: HTMLElement): GraphTheme {
  const doc = el.ownerDocument;
  const view = doc.defaultView ?? globalThis;
  const cs = view.getComputedStyle(el);
  const v = (name: string): string => {
    const s = cs.getPropertyValue(name).trim();
    if (!s) throw new Error(`@cutweave/ui graph editor: missing design token ${name}`);
    return s;
  };
  const px = (name: string): number => Number.parseFloat(v(name));
  const probe = doc.createElement("span");
  probe.className = "numeric";
  probe.style.position = "absolute";
  probe.style.visibility = "hidden";
  el.appendChild(probe);
  const ps = view.getComputedStyle(probe);
  const fontPx = Number.parseFloat(ps.fontSize);
  const font = `${ps.fontWeight} ${ps.fontSize} ${ps.fontFamily}`;
  probe.remove();
  if (!Number.isFinite(fontPx)) {
    throw new Error("@cutweave/ui graph editor: type style .numeric missing");
  }
  return {
    surface: v("--surface-0"),
    ruler: v("--surface-1"),
    grid: v("--border-subtle"),
    gridStrong: v("--border-default"),
    text: v("--text-muted"),
    textStrong: v("--text-primary"),
    playhead: v("--playhead"),
    selected: v("--marigold"),
    onSelected: v("--on-marigold"),
    marquee: v("--marigold-subtle"),
    guide: v("--overlay-guide"),
    curves: CURVE_TOKENS.map((name) => v(name)),
    font,
    fontPx,
    dimmed: Number.parseFloat(v("--opacity-disabled")),
    line: px("--border-thin"),
    lineStrong: px("--border-thick"),
    rulerHeight: px("--control-sm"),
    rulerWidth: px("--space-12") + px("--space-4"),
    keySize: px("--space-2"),
    handleSize: px("--space-1-5"),
    hitRadius: px("--space-2"),
    pad: px("--space-1"),
  };
}
