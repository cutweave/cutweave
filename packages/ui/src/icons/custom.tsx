// SPDX-License-Identifier: GPL-3.0-or-later
import type { ReactNode, SVGProps } from "react";
import type { SvgComponent } from "./lucide-set.ts";

/**
 * Cutweave editor glyphs, drawn on Lucide's 24-unit grid (round caps and joins).
 * Stroke width, colour and size are applied by `<Icon>`; shapes use only `currentColor`.
 */
function glyph(children: ReactNode): SvgComponent {
  return function Glyph(props: SVGProps<SVGSVGElement>) {
    return (
      // biome-ignore lint/a11y/noSvgWithoutTitle: <Icon> sets aria-hidden or role="img" + aria-label.
      <svg
        xmlns="http://www.w3.org/2000/svg"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth={1.75}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        {children}
      </svg>
    );
  };
}

const DIAMOND = "M12 3.5 20.5 12 12 20.5 3.5 12Z";

export const CUSTOM_ICONS = {
  /** Razor / blade tool: a blade across a cut line. */
  razor: glyph(
    <>
      <path d="M12 3v2M12 19v2" />
      <path d="M4 9h16l-2.5 6h-11Z" />
      <path d="M12 9v6" />
    </>,
  ),
  /** Ripple edit: trim edge that pushes following clips. */
  ripple: glyph(
    <>
      <path d="M8 4v16" />
      <path d="M8 12h7" />
      <path d="m12 8 4 4-4 4" />
      <path d="M20 5v14" />
    </>,
  ),
  /** Roll edit: both sides of a cut move together. */
  roll: glyph(
    <>
      <path d="M12 4v16" />
      <path d="m8 9-4 3 4 3" />
      <path d="m16 9 4 3-4 3" />
    </>,
  ),
  /** Slip: content moves inside a fixed clip window. */
  slip: glyph(
    <>
      <rect x="6" y="6" width="12" height="12" rx="1.5" />
      <path d="m10 10-2 2 2 2" />
      <path d="m14 10 2 2-2 2" />
    </>,
  ),
  /** Slide: clip moves between neighbours. */
  slide: glyph(
    <>
      <path d="M3 8v8M21 8v8" />
      <rect x="8" y="7" width="8" height="10" rx="1.5" />
      <path d="M3 12h5M16 12h5" />
    </>,
  ),
  /** Magnetic track / snapping. */
  magnet: glyph(
    <>
      <path d="M5 4v8a7 7 0 0 0 14 0V4" />
      <path d="M5 8h4M15 8h4" />
    </>,
  ),
  /** Keyframe, empty diamond (no keyframe at playhead). */
  "keyframe-diamond": glyph(<path d={DIAMOND} />),
  /** Keyframe, filled diamond (keyframe at playhead). */
  "keyframe-diamond-filled": glyph(<path d={DIAMOND} fill="currentColor" />),
  /** Keyframe, half-filled diamond (mixed state). */
  "keyframe-diamond-half": glyph(
    <>
      <path d={DIAMOND} />
      <path d="M12 3.5 3.5 12 12 20.5Z" fill="currentColor" />
    </>,
  ),
  /** Interpolation: hold (the value jumps at the next key). */
  "ease-hold": glyph(<path d="M4 18h12V6h4" />),
  /** Interpolation: linear (constant speed). */
  "ease-linear": glyph(<path d="M4 18 20 6" />),
  /** Interpolation: bezier (eases out of the key and into the next). */
  "ease-bezier": glyph(<path d="M4 18c8 0 8-12 16-12" />),
  /** Interpolation: spring (overshoots, then settles). */
  "ease-spring": glyph(<path d="M4 18c3 0 3-15 7-15s2 6 4 6 2-3 5-3" />),
  /** Mark in point. */
  "mark-in": glyph(
    <>
      <path d="M9 4v16" />
      <path d="M9 4h8" />
      <path d="M9 20h8" />
      <path d="M13 12h8" />
      <path d="m18 9 3 3-3 3" />
    </>,
  ),
  /** Mark out point. */
  "mark-out": glyph(
    <>
      <path d="M15 4v16" />
      <path d="M15 4H7" />
      <path d="M15 20H7" />
      <path d="M3 12h8" />
      <path d="m8 9 3 3-3 3" />
    </>,
  ),
  /** Agent proposal (dashed outline with a tick; sparkles stays reserved for AI). */
  proposal: glyph(
    <>
      <rect x="4" y="4" width="16" height="16" rx="3" strokeDasharray="3 3" />
      <path d="m9 12 2.2 2.2L15.5 9.8" />
    </>,
  ),
} as const satisfies Record<string, SvgComponent>;

export type CustomIconName = keyof typeof CUSTOM_ICONS;
