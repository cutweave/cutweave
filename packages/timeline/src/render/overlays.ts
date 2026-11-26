// SPDX-License-Identifier: GPL-3.0-or-later
// Overlays over the lanes: the in/out range shade, snap guides (colored by the kind of target
// they landed on, WP-2A.07), the marquee and the playhead line.
import { NO_DASH } from "./clips.ts";
import { GUIDE_KINDS, type RenderState } from "./state.ts";

export function drawOverlays(s: RenderState): void {
  const { ctx, f, rx0, rx1 } = s;
  const { vp, theme: th, view } = f;
  const top = vp.rulerHeight;
  const bottom = vp.height;

  const range = view.range;
  if (range) {
    const a = Math.round(s.x(range.in));
    const b = Math.round(s.x(range.out));
    ctx.fillStyle = th.marigoldSubtle;
    ctx.fillRect(a, top, b - a, bottom - top);
    s.stats.drawCalls++;
  }

  const guides = view.snapGuides;
  if (guides.length > 0) {
    // One batch per kind: a line through the lanes and a small cap where the lanes begin.
    const cap = th.space1;
    for (let kind = 0; kind < GUIDE_KINDS.length; kind++) {
      const name = GUIDE_KINDS[kind];
      let any = false;
      ctx.beginPath();
      for (let n = 0; n < guides.length; n++) {
        const g = guides[n];
        if (g === undefined) continue;
        const gKind = typeof g === "number" ? "edge" : g.kind;
        if (gKind !== name) continue;
        const x = Math.round(s.x(typeof g === "number" ? g : g.t)) + 0.5;
        if (x < rx0 - 1 || x > rx1 + 1 || x < vp.headerWidth) continue;
        ctx.moveTo(x, top);
        ctx.lineTo(x, bottom);
        any = true;
      }
      if (!any) continue;
      const style = s.guideStyle[kind];
      if (style === undefined) continue;
      ctx.strokeStyle = style.color;
      ctx.lineWidth = th.borderThin;
      ctx.setLineDash(style.dashed ? s.dashGuide : NO_DASH);
      ctx.stroke();
      ctx.setLineDash(NO_DASH);
      // Caps: a downward triangle at the top of the lanes, in the guide's color.
      ctx.beginPath();
      for (let n = 0; n < guides.length; n++) {
        const g = guides[n];
        if (g === undefined) continue;
        if ((typeof g === "number" ? "edge" : g.kind) !== name) continue;
        const x = Math.round(s.x(typeof g === "number" ? g : g.t)) + 0.5;
        if (x < rx0 - 1 || x > rx1 + 1 || x < vp.headerWidth) continue;
        ctx.moveTo(x - cap, top);
        ctx.lineTo(x + cap, top);
        ctx.lineTo(x, top + cap);
        ctx.closePath();
      }
      ctx.fillStyle = style.color;
      ctx.fill();
      s.stats.drawCalls += 2;
    }
  }

  const m = view.marquee;
  if (m) {
    ctx.fillStyle = th.marigoldSubtle;
    ctx.fillRect(m.x, m.y, m.w, m.h);
    ctx.strokeStyle = th.marigold;
    ctx.lineWidth = th.borderThin;
    ctx.strokeRect(m.x + 0.5, m.y + 0.5, m.w - 1, m.h - 1);
    s.stats.drawCalls += 2;
  }

  const x = Math.round(s.x(view.playhead));
  if (x >= vp.headerWidth - 1 && x <= vp.width + 1) {
    ctx.fillStyle = th.playhead;
    ctx.fillRect(x - th.space0_5 / 2, top, th.space0_5, bottom - top);
    s.stats.drawCalls++;
  }
}
