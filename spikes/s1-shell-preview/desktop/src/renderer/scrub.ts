// SPDX-License-Identifier: GPL-3.0-or-later
// Scrub bar + timecode (WP-0C.02). Pointer scrubbing sends latest-wins seek requests: one per
// dispatched pointermove (Chromium aligns them to animation frames), skipped when the frame
// did not change. Each request carries the time of the earliest input event it answers
// (coalesced events included), which is where scrub-to-pixel latency starts.

import { FLICKS_PER_SECOND } from "../shared/types.ts";

export function timecode(pts: number, fps: number): string {
  const frames = Math.max(0, Math.round((pts / FLICKS_PER_SECOND) * fps));
  const p2 = (n: number) => String(n).padStart(2, "0");
  const ff = frames % fps;
  const s = Math.floor(frames / fps);
  return `${p2(Math.floor(s / 3600))}:${p2(Math.floor(s / 60) % 60)}:${p2(s % 60)}:${p2(ff)}`;
}

export interface ScrubCallbacks {
  /** Pointer scrub begins (pause playback). */
  start(): void;
  /** Seek request for frame-aligned `pts`; `t0` = earliest input event time it answers. */
  seek(pts: number, t0: number): void;
  /** Pointer scrub ends (resume playback if it was playing). */
  end(): void;
}

export class ScrubBar {
  scrubbing = false;
  /** Seek requests sent by pointer or keyboard. */
  requests = 0;
  private pointerId = -1;
  private lastSent = Number.NaN;
  private shown = Number.NaN;
  private width = 1;
  private readonly frameFlicks: number;

  constructor(
    private readonly track: HTMLElement,
    private readonly playhead: HTMLElement,
    private readonly tc: HTMLElement,
    durEl: HTMLElement,
    readonly durationFlicks: number,
    private readonly fps: number,
    private readonly cb: ScrubCallbacks,
  ) {
    this.frameFlicks = FLICKS_PER_SECOND / fps;
    durEl.textContent = timecode(durationFlicks, fps);
    track.setAttribute("aria-valuemin", "0");
    track.setAttribute("aria-valuemax", String(durationFlicks / FLICKS_PER_SECOND));
    new ResizeObserver(() => {
      this.width = Math.max(1, track.getBoundingClientRect().width);
      const pts = this.shown;
      this.shown = Number.NaN;
      if (Number.isFinite(pts)) this.show(pts);
    }).observe(track);
    track.addEventListener("pointerdown", this.onDown);
    track.addEventListener("pointermove", this.onMove);
    track.addEventListener("pointerup", this.onUp);
    track.addEventListener("pointercancel", this.onUp);
    track.addEventListener("keydown", this.onKey);
  }

  /** Playback position from the presented frame (ignored while the pointer scrubs). */
  setPosition(pts: number): void {
    if (!this.scrubbing) this.show(pts);
  }

  private show(pts: number): void {
    if (pts === this.shown) return;
    this.shown = pts;
    const frac = Math.min(1, Math.max(0, pts / this.durationFlicks));
    this.playhead.style.transform = `translateX(${frac * this.width}px)`;
    const tc = timecode(pts, this.fps);
    this.tc.textContent = tc;
    this.track.setAttribute("aria-valuenow", (pts / FLICKS_PER_SECOND).toFixed(3));
    this.track.setAttribute("aria-valuetext", tc);
  }

  private ptsAt(clientX: number): number {
    const r = this.track.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    return Math.round((frac * this.durationFlicks) / this.frameFlicks) * this.frameFlicks;
  }

  private request(pts: number, t0: number): void {
    const p = Math.min(this.durationFlicks, Math.max(0, pts));
    this.show(p);
    if (p === this.lastSent) return;
    this.lastSent = p;
    this.requests++;
    this.cb.seek(p, t0);
  }

  private readonly onDown = (e: PointerEvent): void => {
    if (e.button !== 0) return;
    this.track.setPointerCapture(e.pointerId);
    this.pointerId = e.pointerId;
    this.scrubbing = true;
    this.lastSent = Number.NaN;
    this.cb.start();
    this.request(this.ptsAt(e.clientX), e.timeStamp);
    e.preventDefault();
  };

  private readonly onMove = (e: PointerEvent): void => {
    if (!this.scrubbing || e.pointerId !== this.pointerId) return;
    const first = e.getCoalescedEvents?.()[0];
    this.request(this.ptsAt(e.clientX), Math.min(e.timeStamp, first?.timeStamp ?? e.timeStamp));
  };

  private readonly onUp = (e: PointerEvent): void => {
    if (!this.scrubbing || e.pointerId !== this.pointerId) return;
    this.scrubbing = false;
    this.pointerId = -1;
    if (this.track.hasPointerCapture(e.pointerId)) this.track.releasePointerCapture(e.pointerId);
    this.cb.end();
  };

  private readonly onKey = (e: KeyboardEvent): void => {
    const cur = Number.isFinite(this.shown) ? this.shown : 0;
    const step = e.shiftKey ? FLICKS_PER_SECOND : this.frameFlicks;
    const next: Record<string, number> = {
      ArrowLeft: cur - step,
      ArrowRight: cur + step,
      Home: 0,
      End: this.durationFlicks,
    };
    const v = next[e.key];
    if (v === undefined) return;
    this.lastSent = Number.NaN;
    this.request(v, e.timeStamp);
    e.preventDefault();
  };
}
