// SPDX-License-Identifier: GPL-3.0-or-later
// Feasibility probe (WP-0C.06): GPU texture handoff to a FULLY SANDBOXED renderer with Electron's
// experimental `sharedTexture` module (Electron >= 39; pinned 45.4.1). `--transport=texture`.
//
// The real producer would be the engine exporting a D3D11/D3D12 (NT handle), IOSurface or
// DMA-BUF texture that main imports. Building that interop is out of scope for 0C.06, so this
// probe uses an offscreen (OSR) window with `useSharedTexture` as a stand-in GPU frame source:
// it renders a 60 fps animation and each `paint` event yields a shared texture handle. Main then
// does exactly what the product path would do:
//   sharedTexture.importSharedTexture(textureInfo) -> sendSharedTexture(consumer main frame)
// and the consumer's sandboxed preload receives it with `setSharedTextureReceiver`, turns it
// into a `VideoFrame` and transfers that to the page, which draws it with WebGPU
// `importExternalTexture`. No pixel crosses a process boundary on the CPU.
// What this does NOT prove: the engine-side export (wgpu interop) and same-adapter selection
// on hybrid GPUs (README finding 5); the OSR producer is Chromium's own GPU process.

import path from "node:path";
import { app, BrowserWindow, type IpcMainInvokeEvent, ipcMain, sharedTexture } from "electron";
import { runBench, waitFor } from "../bench.ts";

export interface TextureProbeOptions {
  origin: string;
  spikeRoot: string;
  size: string;
  benchSeconds: number | null;
  selftestSeconds: number | null;
  warmupSeconds: number;
  options: Record<string, unknown>;
}

const nowUs = () => (performance.timeOrigin + performance.now()) * 1000;

export async function startTextureProbe(o: TextureProbeOptions): Promise<void> {
  const [w, h] = o.size.split("x").map(Number) as [number, number];
  const consumer = new BrowserWindow({
    width: 1280,
    height: 820,
    backgroundColor: "#0e0f12",
    title: "Cutweave S1 spike: sharedTexture probe",
    webPreferences: {
      preload: path.join(__dirname, "preload-texture.cjs"),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false,
    },
  });
  const isConsumer = (e: IpcMainInvokeEvent) =>
    e.sender === consumer.webContents && e.senderFrame === e.sender.mainFrame;
  ipcMain.handle("s1t:now", (e) => (isConsumer(e) ? nowUs() : null));
  consumer.webContents.on("console-message", (e) => {
    if (e.level === "error" || e.level === "warning") console.log(`[texture page] ${e.message}`);
  });
  await consumer.loadURL(`${o.origin}/b/texture.html?policy=${String(o.options.policy ?? "latest")}`);

  const producer = new BrowserWindow({
    width: w,
    height: h,
    show: false,
    useContentSize: true,
    enableLargerThanScreen: true, // keep the full 1920x1080 on a 1080p panel with a taskbar
    webPreferences: {
      offscreen: { useSharedTexture: true, sharedTexturePixelFormat: "argb", deviceScaleFactor: 1 },
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      backgroundThrottling: false,
    },
  });
  producer.setContentSize(w, h); // the constructor clamps to the work area
  producer.webContents.setFrameRate(60);

  const stats = { paints: 0, sent: 0, busyDrops: 0, errors: 0, importMs: 0, sendMs: 0 };
  let statsSince = performance.now();
  let busy = false;
  let seq = 0;
  producer.webContents.on("paint", (e) => {
    const tex = e.texture;
    if (!tex) return;
    stats.paints++;
    if (busy) {
      // Latest-wins: the consumer has not taken the previous frame yet.
      stats.busyDrops++;
      tex.release();
      return;
    }
    busy = true;
    const tPaintUs = nowUs();
    const t0 = performance.now();
    let imported: Electron.SharedTextureImported;
    try {
      imported = sharedTexture.importSharedTexture({
        textureInfo: tex.textureInfo,
        allReferencesReleased: () => tex.release(),
      });
    } catch (err) {
      stats.errors++;
      tex.release();
      busy = false;
      console.error(`[texture] import failed: ${(err as Error).message}`);
      return;
    }
    const t1 = performance.now();
    stats.importMs += t1 - t0;
    const meta = { seq: ++seq, tPaintUs, tSendUs: nowUs(), width: w, height: h };
    sharedTexture
      .sendSharedTexture({ frame: consumer.webContents.mainFrame, importedSharedTexture: imported }, meta)
      .then(
        () => {
          stats.sent++;
          stats.sendMs += performance.now() - t1;
        },
        (err: Error) => {
          stats.errors++;
          console.error(`[texture] send failed: ${err.message}`);
        },
      )
      .finally(() => {
        imported.release();
        busy = false;
      });
  });
  await producer.loadURL(`${o.origin}/b/producer.html`);
  if (process.env.S1T_DEBUG) {
    setInterval(() => console.log(`[texture] ${JSON.stringify(stats)}`), 1000).unref();
  }

  const extra = async (reset: boolean) => {
    const snapshot = {
      ...stats,
      paintsPerSecond: Math.round((stats.paints / ((performance.now() - statsSince) / 1000)) * 10) / 10,
      importMsMean: stats.sent ? stats.importMs / stats.sent : null,
      sendMsMean: stats.sent ? stats.sendMs / stats.sent : null,
      producerPid: producer.webContents.getOSProcessId(),
    };
    if (reset) {
      for (const k of Object.keys(stats) as (keyof typeof stats)[]) stats[k] = 0;
      statsSince = performance.now();
    }
    return { texture: snapshot };
  };
  const finish = (ok: boolean) => app.exit(ok ? 0 : 1);

  const seconds = o.benchSeconds ?? o.selftestSeconds;
  if (seconds !== null) {
    if (o.selftestSeconds !== null) {
      // Minimal checks: frames arrive in a sandboxed renderer through the GPU path.
      await waitFor(
        "texture frames",
        () => consumer.webContents.executeJavaScript("window.__s1?.framesPresented ?? 0"),
        20_000,
      );
    }
    runBench({
      win: consumer,
      transport: "texture",
      seconds,
      warmupSeconds: o.warmupSeconds,
      engineInfo: () => null,
      enginePid: () => null,
      utilityPid: () => null,
      producerPid: () => producer.webContents.getOSProcessId(),
      options: o.options,
      spikeRoot: o.spikeRoot,
      extra,
    }).then(finish, (err: Error) => {
      console.error(`[bench] error: ${err.stack ?? err.message}`);
      finish(false);
    });
  }
}
