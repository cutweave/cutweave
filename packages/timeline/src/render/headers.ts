// SPDX-License-Identifier: GPL-3.0-or-later
// Track headers: sticky at the left edge (they never scroll horizontally) and scrolled with the
// lanes vertically. Kind icon, name and the lock / mute / hide toggles (pressed: marigold-subtle
// fill, marigold-text icon); the magnetic main track gets a marigold edge.
import { HEADER_TOGGLES, type HeaderToggle, nameRect, toggleRect } from "../geometry.ts";
import type { TimelineTrack } from "../model.ts";
import { drawGlyph, KIND_ICON, TOGGLE_ICON } from "./glyphs.ts";
import type { RenderState } from "./state.ts";

export function drawHeaders(s: RenderState, y0: number, y1: number): void {
  const { ctx, f } = s;
  const { layout, vp, theme: th } = f;
  const tracks = layout.model.tracks;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  for (let ti = 0; ti < tracks.length; ti++) {
    const laneTop = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY;
    const laneH = layout.trackHeight[ti] ?? 0;
    if (laneTop + laneH <= y0 || laneTop >= y1) continue;
    const track = tracks[ti] as TimelineTrack;
    ctx.fillStyle = th.surface1;
    ctx.fillRect(0, laneTop, vp.headerWidth, laneH);
    ctx.fillStyle = th.borderSubtle;
    ctx.fillRect(0, laneTop + laneH - th.borderThin, vp.headerWidth, th.borderThin);
    if (track.main) {
      ctx.fillStyle = th.marigold;
      ctx.fillRect(0, laneTop, th.space0_5, laneH);
    }
    const cy = laneTop + laneH / 2;
    if (
      drawGlyph(
        ctx,
        s.glyphs,
        KIND_ICON[track.kind],
        th.space2 + th.iconSm / 2,
        cy,
        th.iconSm,
        th.clip[track.kind].strong,
      )
    ) {
      s.stats.drawCalls++;
    }

    const nameBox = nameRect(vp, th, laneTop, laneH, s.rect);
    const text = s.trackNames.fit(
      layout,
      tracks.length,
      ti,
      track.name,
      nameBox.w,
      s.charLabel,
    );
    ctx.font = th.fontLabel;
    ctx.fillStyle = th.textPrimary;
    if (text !== "") ctx.fillText(text, nameBox.x, cy);
    s.stats.drawCalls += 4;

    for (let i = 0; i < HEADER_TOGGLES.length; i++) {
      const name = HEADER_TOGGLES[i] as HeaderToggle;
      const r = toggleRect(vp, th, laneTop, laneH, i, s.rect);
      const pressed = name === "lock" ? track.locked : name === "mute" ? track.muted : track.hidden;
      if (pressed) {
        ctx.fillStyle = th.marigoldSubtle;
        ctx.beginPath();
        ctx.roundRect(r.x, r.y, r.w, r.h, th.radiusSm);
        ctx.fill();
        s.stats.drawCalls++;
      }
      if (
        drawGlyph(
          ctx,
          s.glyphs,
          TOGGLE_ICON[name],
          r.x + r.w / 2,
          r.y + r.h / 2,
          th.iconSm,
          pressed ? th.marigoldText : th.textSecondary,
        )
      ) {
        s.stats.drawCalls++;
      }
    }
  }
}
