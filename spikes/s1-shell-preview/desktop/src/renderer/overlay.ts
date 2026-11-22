// SPDX-License-Identifier: GPL-3.0-or-later
// DOM overlay above the preview canvas (WP-0C.02, arch §10 step 4): a draggable/resizable
// transform box and safe-area guides. The box lives in normalised sequence coordinates
// (0..1 of the frame) and is laid out with percentages of the overlay, which has exactly the
// canvas's box, so it stays aligned through window resizes without any per-frame JS.

export interface Rect01 {
  x: number;
  y: number;
  w: number;
  h: number;
}

export const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;
export type Handle = (typeof HANDLES)[number];

const MIN_SIZE = 0.02;
/** Arrow-key step: one pixel of a 1920 x 1080 sequence (Shift: ten). */
const KEY_STEP_X = 1 / 1920;
const KEY_STEP_Y = 1 / 1080;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

export class TransformBox {
  rect: Rect01 = { x: 0.3, y: 0.3, w: 0.4, h: 0.4 };
  readonly handles = new Map<Handle, HTMLElement>();
  /** Completed pointer gestures (moves + resizes), for the self-test. */
  gestures = 0;
  private drag: {
    id: number;
    kind: "move" | Handle;
    x0: number;
    y0: number;
    start: Rect01;
  } | null = null;

  constructor(
    readonly overlay: HTMLElement,
    readonly el: HTMLElement,
  ) {
    for (const h of HANDLES) {
      const d = document.createElement("div");
      d.className = "handle";
      d.dataset.h = h;
      el.append(d);
      this.handles.set(h, d);
    }
    el.addEventListener("pointerdown", this.onDown);
    el.addEventListener("pointermove", this.onMove);
    el.addEventListener("pointerup", this.onUp);
    el.addEventListener("pointercancel", this.onUp);
    el.addEventListener("keydown", this.onKey);
    // Pointer down on empty overlay area deselects (the canvas below never gets events).
    overlay.addEventListener("pointerdown", (e) => {
      if (e.target === overlay) this.select(false);
    });
    this.select(true);
    this.apply();
  }

  select(on: boolean): void {
    this.el.classList.toggle("selected", on);
  }

  apply(): void {
    const r = this.rect;
    const s = this.el.style;
    s.left = `${r.x * 100}%`;
    s.top = `${r.y * 100}%`;
    s.width = `${r.w * 100}%`;
    s.height = `${r.h * 100}%`;
  }

  private readonly onDown = (e: PointerEvent): void => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    const kind = (target.dataset.h as Handle | undefined) ?? "move";
    this.drag = { id: e.pointerId, kind, x0: e.clientX, y0: e.clientY, start: { ...this.rect } };
    this.el.setPointerCapture(e.pointerId);
    this.select(true);
    this.el.focus({ preventScroll: true });
    e.preventDefault();
    e.stopPropagation();
  };

  private readonly onMove = (e: PointerEvent): void => {
    const d = this.drag;
    if (!d || e.pointerId !== d.id) return;
    const box = this.overlay.getBoundingClientRect();
    const dx = (e.clientX - d.x0) / box.width;
    const dy = (e.clientY - d.y0) / box.height;
    const s = d.start;
    const r = { ...s };
    if (d.kind === "move") {
      r.x = clamp(s.x + dx, -s.w + MIN_SIZE, 1 - MIN_SIZE);
      r.y = clamp(s.y + dy, -s.h + MIN_SIZE, 1 - MIN_SIZE);
    } else {
      if (d.kind.includes("w")) {
        const x = Math.min(s.x + dx, s.x + s.w - MIN_SIZE);
        r.w = s.x + s.w - x;
        r.x = x;
      }
      if (d.kind.includes("e")) r.w = Math.max(MIN_SIZE, s.w + dx);
      if (d.kind.includes("n")) {
        const y = Math.min(s.y + dy, s.y + s.h - MIN_SIZE);
        r.h = s.y + s.h - y;
        r.y = y;
      }
      if (d.kind.includes("s")) r.h = Math.max(MIN_SIZE, s.h + dy);
    }
    this.rect = r;
    this.apply();
  };

  private readonly onUp = (e: PointerEvent): void => {
    if (!this.drag || e.pointerId !== this.drag.id) return;
    this.drag = null;
    this.gestures++;
    if (this.el.hasPointerCapture(e.pointerId)) this.el.releasePointerCapture(e.pointerId);
  };

  private readonly onKey = (e: KeyboardEvent): void => {
    const k = e.shiftKey ? 10 : 1;
    const d: Record<string, [number, number]> = {
      ArrowLeft: [-k * KEY_STEP_X, 0],
      ArrowRight: [k * KEY_STEP_X, 0],
      ArrowUp: [0, -k * KEY_STEP_Y],
      ArrowDown: [0, k * KEY_STEP_Y],
    };
    const v = d[e.key];
    if (!v) return;
    this.rect = { ...this.rect, x: this.rect.x + v[0], y: this.rect.y + v[1] };
    this.apply();
    e.preventDefault();
  };
}
