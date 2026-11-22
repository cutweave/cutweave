// SPDX-License-Identifier: GPL-3.0-or-later
// sharedTexture probe frame source (WP-0C.06): rendered OFFSCREEN with `useSharedTexture`, so
// every rAF repaint becomes a GPU texture in main's `paint` event. A moving pattern + counter
// forces a full repaint each frame (stand-in for the engine's 60 fps preview).

const canvas = document.getElementById("c") as HTMLCanvasElement;
const ctx = canvas.getContext("2d", { alpha: false });
let n = 0;

function frame(): void {
  if (!ctx) return;
  const w = window.innerWidth;
  const h = window.innerHeight;
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  n++;
  const hue = (n * 2) % 360;
  ctx.fillStyle = `hsl(${hue} 40% 18%)`;
  ctx.fillRect(0, 0, w, h);
  const bars = 16;
  for (let i = 0; i < bars; i++) {
    ctx.fillStyle = `hsl(${(hue + i * 22) % 360} 70% 55%)`;
    const x = ((n * 8 + (i * w) / bars) % (w + 120)) - 120;
    ctx.fillRect(x, (i * h) / bars, 120, h / bars - 4);
  }
  ctx.fillStyle = "#fff";
  ctx.font = `${Math.round(h / 8)}px ui-monospace, Consolas, monospace`;
  ctx.fillText(`frame ${n}`, w * 0.05, h * 0.55);
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
