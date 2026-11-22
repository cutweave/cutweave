// SPDX-License-Identifier: GPL-3.0-or-later
// Preview = presenter + present loop + stage (WP-0C.02). Built by whichever world runs the
// loop: the preload's isolated world (draw=preload, the WP-0C.01 recommendation) or the
// renderer main world (draw=main). The page's main world drives it through `PreviewControls`
// (exposed as `window.bridge.preloadLoop` in preload mode); only small objects cross.

import { type LoopSnapshot, type PresentedInfo, PresentLoop } from "./loop.ts";
import { createPresenter, type UploadPath } from "./present.ts";
import { CanvasStage, type EngineSizeMode, type StageInfo } from "./stage.ts";
import type { Frame, FramePolicy } from "./types.ts";

export type { EngineSizeMode, PresentedInfo, StageInfo, UploadPath };

export interface PreviewOptions {
  policy: FramePolicy;
  verify: boolean;
  preferGpu: boolean;
  upload?: UploadPath;
  engineSize?: EngineSizeMode;
}

export interface PreviewDeps {
  drawIn: string;
  getFrame(afterSeq: number, policy: FramePolicy): Frame | null;
  nowUs(): number;
  sendSeek(ptsFlicks: number): void;
  sendResize(width: number, height: number): void;
}

/** Per-second samples for the metrics log, drained by each call. */
export interface PreviewSamples {
  scrubPixelMs: number[];
  renegotiateMs: number[];
}

/** What the main world may call (all arguments and results are small plain values). */
export interface PreviewControls {
  snapshot(recentN?: number): LoopSnapshot | null;
  reset(): void;
  setVerify(on: boolean): void;
  framesPresented(): number;
  framesSinceUp(): number;
  setUpload(path: UploadPath): void;
  setEngineSize(mode: EngineSizeMode): void;
  /** Latest-wins seek for scrubbing: records the request for scrub-to-pixel, then sends it. */
  scrub(ptsFlicks: number, inputTimeMs: number): void;
  /** Experiment: let rAF wait up to `ms` for a just-requested scrub frame (0 = off). */
  setScrubWait(ms: number): void;
  presented(): PresentedInfo | null;
  takeSamples(): PreviewSamples | null;
  stage(): StageInfo | null;
  describe(): string;
}

export interface Preview extends PreviewControls {
  /** New engine generation (seq restarts). The size is replayed by the control channel. */
  engineRestarted(): void;
}

export async function createPreview(
  canvas: HTMLCanvasElement,
  opts: PreviewOptions,
  deps: PreviewDeps,
): Promise<Preview> {
  const presenter = await createPresenter(canvas, opts.preferGpu);
  if (opts.upload) presenter.upload = opts.upload;
  const loop = new PresentLoop(
    deps.drawIn,
    deps.getFrame,
    deps.nowUs,
    presenter,
    opts.policy,
    opts.verify,
  );
  const stage = new CanvasStage(
    canvas,
    () => presenter.redraw(),
    deps.sendResize,
    opts.engineSize ?? "canvas",
  );
  presenter.onGpuDone = (ms) => loop.sample("gpuDoneMs", ms);
  loop.onPresent = (f, t) => stage.framePresented(f, t);
  stage.start();
  loop.start();
  return {
    snapshot: (n) => ({ ...loop.snapshot(n), presenter: `${presenter.kind} ${presenter.lastPath} (${presenter.adapter})` }),
    reset: () => {
      loop.reset();
      stage.resetStats();
    },
    setVerify: (on) => {
      loop.verify = on;
    },
    framesPresented: () => loop.framesPresented,
    framesSinceUp: () => loop.framesSinceUp,
    setUpload: (path) => {
      presenter.upload = path;
    },
    setEngineSize: (mode) => stage.setMode(mode),
    scrub: (pts, t0) => {
      loop.noteScrub(pts, t0);
      deps.sendSeek(pts);
    },
    setScrubWait: (ms) => {
      loop.scrubWaitMs = Number.isFinite(ms) ? Math.min(Math.max(ms, 0), 12) : 0;
    },
    presented: () => loop.presented(),
    takeSamples: () => ({
      scrubPixelMs: loop.takeScrubSamples(),
      renegotiateMs: stage.takeRenegotiateSamples(),
    }),
    stage: () => stage.info(),
    describe: () => `${presenter.kind} (${presenter.adapter})`,
    engineRestarted: () => loop.engineRestarted(),
  };
}
