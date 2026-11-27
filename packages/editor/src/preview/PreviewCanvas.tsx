// SPDX-License-Identifier: GPL-3.0-or-later
// `<PreviewCanvas>` (WP-1M.04): the frame at the playhead, drawn by one of two paths, with the
// overlay layer on top (arch §10 steps 3-4).
//
// - GPU path (ADR-001 transport (a)): the canvas is handed to the desktop preload, which reads
//   the engine's shared-memory frame ring and draws each frame with WebGPU (`RingPreview`).
// - JPEG path (arch §10 fallback): still frames from the engine's `/media/frame` polled into a
//   2D canvas at <= 15 fps (`JpegPreview`), when WebGPU, the bridge addon or the ring is missing,
//   or when `CUTWEAVE_PREVIEW=jpeg` forces it. A banner says the preview is reduced and why.
//
// The canvas backing store always equals its CSS size times the device pixel ratio (a
// `device-pixel-content-box` ResizeObserver); each resize repaints the last frame at once and
// tells the engine the new size (`view.set_preview_size`, debounced, rounded to even). The
// overlay is DOM in the same box, so it lines up at every size.
import type { RpcClient } from "@cutweave/protocol";
import { useOwnerWindow } from "@cutweave/ui";
import { type CSSProperties, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { engineStore, useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { EmptyState } from "../panels/EmptyState.tsx";
import { observeSize, windowOf } from "../shell/windows.ts";
import { docStore, useDocStore } from "../state/docStore.ts";
import { playbackStore } from "../state/playbackStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { ASPECTS, type AspectId, useViewStore, viewStore } from "../state/viewStore.ts";
import {
  canvasSurface,
  type FrameFetcher,
  httpFrameFetcher,
  JpegPreview,
  windowTicker,
} from "./jpegPreview.ts";
import { PreviewOverlay } from "./PreviewOverlay.tsx";
import type { PlayheadSample } from "./presenter.ts";
import { devicePixelSize, PreviewSizer } from "./previewSize.ts";
import {
  type FailureReason,
  type PreviewBridge,
  parseRingAnnouncement,
  type RingAnnouncement,
  RingPreview,
} from "./ringPreview.ts";
import { SourceStills } from "./SourceStills.tsx";

/** Why the preview runs reduced. */
export type ReducedReason = "forced" | FailureReason;

export type PreviewPath =
  | { readonly kind: "pending" }
  | { readonly kind: "gpu" }
  | { readonly kind: "jpeg"; readonly reason: ReducedReason };

export const REDUCED_TEXT: Readonly<Record<ReducedReason, TKey>> = {
  forced: "preview.reduced.forced",
  webgpu: "preview.reduced.webgpu",
  "no-addon": "preview.reduced.noAddon",
  "no-ring": "preview.reduced.noRing",
  ring: "preview.reduced.ring",
};

/** The slice of `window.cutweave` the preview reads (apps/desktop `shared/api.ts`). */
interface DesktopPreviewWindow {
  readonly cutweave?: {
    readonly app?: { getInfo(): Promise<{ readonly preview?: string }> };
    readonly preview?: PreviewBridge;
    readonly engine?: {
      getConnection(): Promise<{
        readonly httpUrl?: string;
        readonly port?: number;
        readonly token: string;
      } | null>;
    };
  };
}

const desktop = (): DesktopPreviewWindow["cutweave"] =>
  (globalThis as unknown as DesktopPreviewWindow).cutweave;

/** `view.*` and `rpc.hello` are not in the generated command map. */
type UntypedCall = (method: string, params: Record<string, unknown>) => Promise<unknown>;
const untyped = (rpc: RpcClient): UntypedCall => {
  const call = rpc.call as unknown as UntypedCall;
  return (method, params) => call.call(rpc, method, params);
};

/** Whether `CUTWEAVE_PREVIEW=jpeg` forces the reduced path; `undefined` while unknown. */
function useForcedJpeg(): boolean | undefined {
  const [forced, setForced] = useState<boolean | undefined>(undefined);
  useEffect(() => {
    const app = desktop()?.app;
    if (app === undefined) {
      setForced(false);
      return;
    }
    let live = true;
    app
      .getInfo()
      .then((info) => live && setForced(info.preview === "jpeg"))
      .catch(() => live && setForced(false));
    return () => {
      live = false;
    };
  }, []);
  return forced;
}

/**
 * The engine's current ring, asked again with `rpc.hello` (its `shm`); while the connection is
 * down, the ring of the next handshake. `null` when it has none (or none within 15 s).
 */
async function announceRing(): Promise<RingAnnouncement | null> {
  const { rpc, connection } = engineStore.getState();
  if (rpc !== null && connection === "ready") {
    try {
      const hello = (await untyped(rpc)("rpc.hello", {
        client: "cutweave-ui",
        version: "0.0.0",
      })) as { shm?: unknown };
      return parseRingAnnouncement(hello.shm);
    } catch {
      // Fall through: wait for the next handshake.
    }
  }
  const before = engineStore.getState().previewRing;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      off();
      resolve(null);
    }, 15_000);
    const off = engineStore.subscribe((s) => {
      if (s.previewRing !== undefined && s.previewRing !== before) {
        clearTimeout(timer);
        off();
        resolve(s.previewRing);
      }
    });
  });
}

/**
 * Keeps `canvas`'s backing store at its device-pixel size; `onResize` runs right after a change
 * (repaint the cleared canvas, tell the engine), in the same frame. The size and the density are
 * the canvas's own window's (a detached Preview's), and a move of that window to a display with
 * another density resizes it too, so `view.set_preview_size` follows.
 */
function useDeviceSizedCanvas(
  canvas: HTMLCanvasElement | null,
  onResize: (width: number, height: number) => void,
): void {
  const ownerWindow = useOwnerWindow();
  useEffect(() => {
    if (canvas === null || ownerWindow === null) return;
    const win = windowOf(canvas);
    const apply = (width: number, height: number): void => {
      if (width <= 0 || height <= 0) return;
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      onResize(width, height);
    };
    const stop = observeSize(
      canvas,
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry === undefined) return;
        const { width, height } = devicePixelSize(entry, win.devicePixelRatio || 1);
        apply(width, height);
      },
      { box: "device-pixel-content-box" },
    );
    // Another display's density: where the observer does not report device pixels, this does.
    let query: MediaQueryList | null = null;
    const onDensity = (): void => {
      const box = canvas.getBoundingClientRect();
      const dpr = win.devicePixelRatio || 1;
      apply(Math.round(box.width * dpr), Math.round(box.height * dpr));
      watch();
    };
    const watch = (): void => {
      query?.removeEventListener("change", onDensity);
      query = win.matchMedia?.(`(resolution: ${win.devicePixelRatio || 1}dppx)`) ?? null;
      query?.addEventListener("change", onDensity);
    };
    watch();
    return () => {
      stop();
      query?.removeEventListener("change", onDensity);
    };
  }, [canvas, onResize, ownerWindow]);
}

interface PathCanvasProps {
  readonly label: string;
  /** Frames reach the canvas (GPU: presenting from the ring; JPEG: a first frame drawn). */
  readonly onLive: (live: boolean) => void;
}

/**
 * Whether a canvas that moves into another window (its panel detached, or docked back) keeps its
 * WebGPU context. Windows does (research 10, the spike); elsewhere, until a check shows it, the
 * canvas is handed to the preload again after a move.
 */
export function keepsContextWhenMoved(
  platform: string = typeof navigator === "undefined" ? "" : navigator.platform,
): boolean {
  return /^win/i.test(platform);
}

/** The GPU path: the preload presents into this canvas. */
function GpuCanvas({
  label,
  onLive,
  onFail,
  frozen,
}: PathCanvasProps & {
  readonly onFail: (reason: ReducedReason) => void;
  /** Holds the picture (the Inspector draws the source): the ring is not presented. */
  readonly frozen: boolean;
}) {
  const [canvas, setCanvas] = useState<HTMLCanvasElement | null>(null);
  const ringRef = useRef<RingPreview | null>(null);
  const frozenRef = useRef(frozen);
  frozenRef.current = frozen;
  // Where a moved canvas may lose its context, a new window attaches it again (WP-2H.15).
  const ownerWindow = useOwnerWindow();
  const attachedIn = keepsContextWhenMoved() ? null : ownerWindow;
  const sizer = useMemo(
    () =>
      new PreviewSizer({
        send: async (width, height) => {
          const rpc = engineStore.getState().rpc;
          if (rpc !== null) await untyped(rpc)("view.set_preview_size", { width, height });
        },
      }),
    [],
  );

  useEffect(() => {
    const bridge = desktop()?.preview;
    if (canvas === null || bridge === undefined) return;
    let live = true;
    const ring = new RingPreview({
      bridge,
      announce: announceRing,
      onState: (state) => {
        if (!live) return;
        onLive(state.kind === "presenting");
        if (state.kind === "failed") {
          console.warn(`[preview] reduced preview (${state.reason}): ${state.message}`);
          onFail(state.reason);
        }
      },
    });
    ringRef.current = ring;
    if (frozenRef.current) ring.pause();
    void ring.attach(canvas, engineStore.getState().previewRing ?? null).then(() => {
      // A fresh ring renders at the engine's default size until told the canvas size.
      if (live && ring.state.kind === "presenting") sizer.resend();
    });
    // A new engine launch announces a new ring in its handshake.
    const off = engineStore.subscribe((s, prev) => {
      if (s.previewRing !== prev.previewRing && s.previewRing !== undefined) {
        ring.setRing(s.previewRing);
        sizer.resend();
      }
    });
    return () => {
      live = false;
      off();
      ring.detach();
      ringRef.current = null;
    };
    // `attachedIn` is read nowhere: a change re-attaches (macOS and Linux after a move).
  }, [canvas, onFail, onLive, sizer, attachedIn]);

  useEffect(() => () => sizer.dispose(), [sizer]);

  useEffect(() => {
    if (frozen) ringRef.current?.pause();
    else ringRef.current?.resume();
  }, [frozen]);

  const onResize = useCallback(
    (width: number, height: number) => {
      ringRef.current?.redraw();
      sizer.observe(width, height);
    },
    [sizer],
  );
  useDeviceSizedCanvas(canvas, onResize);

  return (
    <canvas
      ref={setCanvas}
      className="cw-preview__canvas"
      role="img"
      aria-label={label}
      data-path="gpu"
    />
  );
}

/** What the JPEG path shows for the playhead: the engine's sample while playing, else the view's. */
function playheadSample(): PlayheadSample {
  const pb = playbackStore.getState();
  if (pb.playing && pb.rate !== 0) {
    return { t: pb.playhead, rate: pb.rate, playing: true, at: pb.sampledAt };
  }
  return { t: viewStore.getState().playhead, rate: 0, playing: false, at: 0 };
}

/** The JPEG path: still frames from `/media/frame` drawn into a 2D canvas. */
function JpegCanvas({
  label,
  onLive,
  fetcher,
  frozen,
}: PathCanvasProps & { readonly fetcher?: FrameFetcher; readonly frozen: boolean }) {
  const { t } = useTranslation();
  const [canvas, setCanvas] = useState<HTMLCanvasElement | null>(null);
  const previewRef = useRef<JpegPreview | null>(null);
  const [noFrames, setNoFrames] = useState(false);
  const frozenRef = useRef(frozen);
  frozenRef.current = frozen;

  useEffect(() => {
    if (canvas === null) return;
    const surface = canvasSurface(canvas);
    if (surface === null) return;
    const source =
      fetcher ??
      httpFrameFetcher({
        connection: async () => {
          const info = await desktop()?.engine?.getConnection();
          if (info === undefined || info === null) return null;
          const httpUrl = info.httpUrl ?? `http://127.0.0.1:${info.port ?? 0}`;
          return { httpUrl, token: info.token };
        },
        sequence: () => selectActiveSequenceId(docStore.getState().project),
      });
    const preview = new JpegPreview({
      fetcher: source,
      surface,
      // On the canvas's own window's frames (a detached Preview keeps playing there).
      ticker: windowTicker(() => canvas),
      playhead: playheadSample,
      size: () => ({ width: canvas.width, height: canvas.height }),
      frameRate: () => {
        const project = docStore.getState().project;
        const id = selectActiveSequenceId(project);
        return id === null ? null : (project?.sequences[id]?.format.frame_rate ?? null);
      },
    });
    previewRef.current = preview;
    if (!frozenRef.current) preview.start();
    // An edit changes the picture under the playhead.
    const offDoc = docStore.subscribe((s, prev) => {
      if (s.project !== prev.project) preview.invalidate();
    });
    const watch = setInterval(() => {
      const stats = preview.stats();
      setNoFrames(stats.framesPresented === 0 && stats.errors > 0);
      onLive(stats.framesPresented > 0);
    }, 250);
    return () => {
      clearInterval(watch);
      offDoc();
      preview.stop();
      previewRef.current = null;
    };
  }, [canvas, fetcher, onLive]);

  // While the source shows (here as stills, or in the Inspector) the timeline's frames stop. A
  // move to another window starts them again there (a frame the old window owed may never come).
  const ownerWindow = useOwnerWindow();
  useEffect(() => {
    const preview = previewRef.current;
    if (preview === null || ownerWindow === null) return;
    preview.stop();
    if (!frozen) {
      preview.invalidate();
      preview.start();
    }
  }, [frozen, ownerWindow]);

  const onResize = useCallback(() => {
    previewRef.current?.redraw();
    previewRef.current?.invalidate();
  }, []);
  useDeviceSizedCanvas(canvas, onResize);

  return (
    <>
      <canvas
        ref={setCanvas}
        className="cw-preview__canvas"
        role="img"
        aria-label={label}
        data-path="jpeg"
      />
      {noFrames ? (
        <div className="cw-preview__notice">
          <EmptyState
            icon="triangle-alert"
            title={t("preview.noFramesTitle")}
            body={t("preview.noFramesBody")}
          />
        </div>
      ) : null}
    </>
  );
}

export interface PreviewCanvasProps {
  /** Reports the path in use (the panel shows the reduced-mode banner). */
  readonly onPath?: (path: PreviewPath) => void;
  /** Frames for the JPEG path (tests); the app fetches `/media/frame`. */
  readonly jpegFetcher?: FrameFetcher;
  /** The Preview shows the source: no timeline overlay, and the JPEG path draws stills of it. */
  readonly source?: boolean;
  /** The picture is held (the Inspector draws the source, WP-1M.15). */
  readonly frozen?: boolean;
}

/** The preview canvas with its overlay. Mount it only while an engine is attached. */
export function PreviewCanvas({
  onPath,
  jpegFetcher,
  source = false,
  frozen = false,
}: PreviewCanvasProps) {
  const { t } = useTranslation();
  const [path, setPath] = useState<PreviewPath>({ kind: "pending" });
  const [live, setLive] = useState(false);
  const forced = useForcedJpeg();
  const ready = useEngineStore((s) => s.connection === "ready");
  const ringKnown = useEngineStore((s) => s.previewRing !== undefined);
  // Whether there is anything to show, not the document: a clip dragged on the timeline changes
  // it on every update (issue #244).
  const empty = useDocStore((s) => {
    const id = selectActiveSequenceId(s.project);
    const sequence = id === null ? undefined : s.project?.sequences[id];
    return !Object.values(sequence?.tracks ?? {}).some((tr) => Object.keys(tr.items).length > 0);
  });

  // Decide once, when the engine has said whether it streams frames.
  useEffect(() => {
    if (path.kind !== "pending" || forced === undefined || !ready || !ringKnown) return;
    if (forced) setPath({ kind: "jpeg", reason: "forced" });
    else if (desktop()?.preview?.isAvailable() !== true)
      setPath({ kind: "jpeg", reason: "no-addon" });
    // No ring to present: do not even create a GPU device.
    else if (engineStore.getState().previewRing === null)
      setPath({ kind: "jpeg", reason: "no-ring" });
    else setPath({ kind: "gpu" });
  }, [path.kind, forced, ready, ringKnown]);

  useEffect(() => onPath?.(path), [path, onPath]);

  const fallBack = useCallback((reason: ReducedReason) => {
    setLive(false);
    setPath({ kind: "jpeg", reason });
  }, []);
  const label = t("preview.canvasLabel");

  return (
    <div
      className="cw-preview__surface"
      data-path={path.kind}
      data-reason={path.kind === "jpeg" ? path.reason : undefined}
      data-live={live}
    >
      {path.kind === "gpu" ? (
        <GpuCanvas key="gpu" label={label} onLive={setLive} onFail={fallBack} frozen={frozen} />
      ) : null}
      {path.kind === "jpeg" ? (
        <JpegCanvas
          key="jpeg"
          label={label}
          onLive={setLive}
          frozen={frozen || source}
          {...(jpegFetcher && { fetcher: jpegFetcher })}
        />
      ) : null}
      {source && path.kind === "jpeg" ? <SourceStills /> : null}
      {source ? null : <PreviewOverlay />}
      {empty && !source ? (
        <div className="cw-preview__notice">
          <EmptyState
            icon="file-video"
            title={t("preview.emptyTitle")}
            body={t("preview.emptyBody")}
          />
        </div>
      ) : null}
    </div>
  );
}

/** The frame's aspect: the active sequence's canvas, else the chosen preset. */
export function usePreviewAspect(): {
  readonly ratio: number;
  readonly style: CSSProperties;
  readonly fromSequence: boolean;
  /** The aspect preset of that shape (the sequence's, when it has one); `null`: none matches. */
  readonly presetId: AspectId | null;
} {
  const aspectId = useViewStore((s) => s.aspect);
  const format = useDocStore((s) => {
    const id = selectActiveSequenceId(s.project);
    return id === null ? undefined : s.project?.sequences[id]?.format;
  });
  return useMemo(() => {
    const fromSequence = format !== undefined && format.width > 0 && format.height > 0;
    let w: number;
    let h: number;
    if (fromSequence) {
      w = format.width;
      h = format.height;
    } else {
      const preset = ASPECTS.find((a) => a.id === aspectId) ?? ASPECTS[0];
      w = preset.width;
      h = preset.height;
    }
    const ratio = w / h;
    const preset = fromSequence
      ? ASPECTS.find((a) => Math.abs(a.width / a.height - ratio) < 1e-3)
      : ASPECTS.find((a) => a.id === aspectId);
    return {
      ratio,
      fromSequence,
      presetId: preset?.id ?? null,
      style: { "--cw-aspect": `${w} / ${h}`, "--cw-ar": String(ratio) } as CSSProperties,
    };
  }, [aspectId, format]);
}
