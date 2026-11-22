// SPDX-License-Identifier: GPL-3.0-or-later
// sharedTexture probe preload (WP-0C.06): SANDBOXED. Receives GPU textures sent by main with
// `sharedTexture.sendSharedTexture`, wraps each in a `VideoFrame` (a GPU-backed handle, no pixel
// copy) and transfers it to the page with `window.postMessage(..., [videoFrame])`. The imported
// texture is released only when the page reports the VideoFrame closed (`s1t:done`), so the
// GPU resource outlives every use. Also gives the page a clock offset to main (for latency).

import { ipcRenderer, sharedTexture } from "electron";

interface Meta {
  seq: number;
  tPaintUs: number;
  tSendUs: number;
  width: number;
  height: number;
}

const epochUs = () => (performance.timeOrigin + performance.now()) * 1000;
let offsetUs = 0;
const live = new Map<number, Electron.SharedTextureImported>();

async function syncClock(): Promise<void> {
  let best = Number.POSITIVE_INFINITY;
  for (let i = 0; i < 15; i++) {
    const t0 = epochUs();
    const main = (await ipcRenderer.invoke("s1t:now")) as number | null;
    const t1 = epochUs();
    if (typeof main === "number" && t1 - t0 < best) {
      best = t1 - t0;
      offsetUs = main - (t0 + t1) / 2;
    }
  }
  window.postMessage({ type: "s1t:clock", offsetUs, rttUs: best }, window.location.origin);
}

sharedTexture.setSharedTextureReceiver(async ({ importedSharedTexture }, ...args: unknown[]) => {
  const meta = args[0] as Meta;
  const tRecvUs = epochUs() + offsetUs;
  try {
    const vf = importedSharedTexture.getVideoFrame();
    live.set(meta.seq, importedSharedTexture);
    window.postMessage({ type: "s1t:frame", meta, tRecvUs, vf }, window.location.origin, [vf]);
  } catch (err) {
    console.error(`s1t receiver: ${(err as Error).message}`);
    importedSharedTexture.release();
  }
});

window.addEventListener("message", (e) => {
  if (e.source !== window || e.origin !== window.location.origin) return;
  if (e.data === "s1t:want-clock") void syncClock();
  const d = e.data as { type?: string; seq?: number };
  if (d.type === "s1t:done" && typeof d.seq === "number") {
    live.get(d.seq)?.release();
    live.delete(d.seq);
  }
});
