// SPDX-License-Identifier: GPL-3.0-or-later
// Unit-test support only (not exported from the package, excluded from the build).
import type { ClipColors, Theme } from "./theme.ts";

/** A fixed theme so tests and the node-side bench need no DOM. */
export function testTheme(): Theme {
  const c = (strong: string, body: string): ClipColors => ({ strong, body });
  return {
    surface0: "#000",
    surface1: "#111",
    surface3: "#222",
    lane: "#161616",
    laneAlt: "#1b1b1b",
    borderSubtle: "#262626",
    textPrimary: "#eee",
    textSecondary: "#aaa",
    textMuted: "#999",
    playhead: "#fb3",
    onMarigold: "#111",
    marigold: "#fb3",
    marigoldSubtle: "#fb32",
    marigoldText: "#fb3",
    agent: "#8af",
    agentSubtle: "#8af2",
    clip: {
      media: c("#47b", "#123"),
      audio: c("#4b8", "#132"),
      graphics: c("#d67", "#312"),
      captions: c("#b8e", "#324"),
      effects: c("#ac4", "#231"),
    },
    fontCaption: "500 11px sans-serif",
    fontLabel: "500 12px sans-serif",
    fontNumeric: "400 12px monospace",
    captionPx: 11,
    numericPx: 12,
    opacityDisabled: 0.5,
    borderThin: 1,
    clipStrip: 3,
    radiusSm: 4,
    controlSm: 24,
    controlMd: 28,
    iconSm: 14,
    trackSm: 32,
    trackMd: 48,
    trackLg: 72,
    space0_5: 2,
    space1: 4,
    space1_5: 6,
    space2: 8,
    space4: 16,
    space12: 48,
  };
}

export interface SpyContext {
  readonly ctx: CanvasRenderingContext2D;
  /** Number of calls per method name. */
  readonly calls: Record<string, number>;
  reset(): void;
}

/** A no-op 2D context that counts every method call (`measureText` is 6 px per character). */
export function spyContext(): SpyContext {
  const calls: Record<string, number> = {};
  const ctx = new Proxy(
    {},
    {
      get(_t, prop) {
        const name = String(prop);
        if (name === "measureText") {
          return (s: string) => {
            calls[name] = (calls[name] ?? 0) + 1;
            return { width: s.length * 6 };
          };
        }
        return () => {
          calls[name] = (calls[name] ?? 0) + 1;
        };
      },
      set() {
        return true;
      },
    },
  ) as CanvasRenderingContext2D;
  return {
    ctx,
    calls,
    reset() {
      for (const k of Object.keys(calls)) delete calls[k];
    },
  };
}

/** The slice of `HTMLCanvasElement` that `TimelineView` touches when given a context and theme. */
export function fakeCanvas(): HTMLCanvasElement {
  return { width: 0, height: 0, style: {} } as unknown as HTMLCanvasElement;
}

/** Manual frame scheduler: `run()` fires the pending callback, like one animation frame. */
export function manualFrames(): {
  requestFrame: (cb: () => void) => number;
  cancelFrame: (id: number) => void;
  run: () => boolean;
  readonly pending: boolean;
} {
  let cb: (() => void) | null = null;
  return {
    requestFrame: (fn) => {
      cb = fn;
      return 1;
    },
    cancelFrame: () => {
      cb = null;
    },
    run: () => {
      const fn = cb;
      cb = null;
      fn?.();
      return fn !== null;
    },
    get pending() {
      return cb !== null;
    },
  };
}
