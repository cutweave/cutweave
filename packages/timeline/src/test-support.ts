// SPDX-License-Identifier: GPL-3.0-or-later
// Unit-test support only (not exported from the package, excluded from the build).
import type { ClipColors, Theme } from "./theme.ts";

/** A fixed theme so tests need no DOM (values are token references; the fake canvas ignores them). */
export function testTheme(): Theme {
  const c = (strong: string, body: string): ClipColors => ({ strong, body });
  return {
    surface0: "var(--surface-0)",
    surface1: "var(--surface-1)",
    surface3: "var(--surface-3)",
    lane: "var(--timeline-lane)",
    laneAlt: "var(--timeline-lane-alt)",
    borderSubtle: "var(--border-subtle)",
    textPrimary: "var(--text-primary)",
    textSecondary: "var(--text-secondary)",
    textMuted: "var(--text-muted)",
    playhead: "var(--playhead)",
    onMarigold: "var(--on-marigold)",
    marigold: "var(--marigold)",
    marigoldSubtle: "var(--marigold-subtle)",
    marigoldText: "var(--marigold-text)",
    agent: "var(--agent)",
    agentSubtle: "var(--agent-subtle)",
    clip: {
      media: c("var(--clip-media)", "var(--clip-media-body)"),
      audio: c("var(--clip-audio)", "var(--clip-audio-body)"),
      graphics: c("var(--clip-graphics)", "var(--clip-graphics-body)"),
      captions: c("var(--clip-captions)", "var(--clip-captions-body)"),
      effects: c("var(--clip-effects)", "var(--clip-effects-body)"),
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
