// SPDX-License-Identifier: GPL-3.0-or-later
// Puts the engine's media on screen (WP-1I.03): once the engine connection is up and the engine
// has a media backend (`media.capabilities`), the media bin's cards get real thumbnails
// (`HttpThumbnailSource`, installed with `setThumbnailSource`) and the timeline gets its media
// loaders (`mediaLoadersStore`, read by the timeline panel): filmstrips on video clips and
// waveforms on audio clips. An engine without media (`--dev-stub-media`, an engine built without
// FFmpeg) keeps the placeholders.
//
// - Every handshake asks again: a restarted engine signs with a new key, so the signed URLs are
//   dropped (`reset`), and it may have media where the last one had none.
// - While an asset's `thumbnails` analysis job runs, its filmstrip is not asked for (the route
//   would generate the same strip a second time, competing with playback for the CPU); when
//   a `thumbnails` or `waveform` job ends the timeline gets fresh loaders and asks again.
import type { MediaId, MediaKind } from "@cutweave/protocol";
import type { MediaLoaders } from "@cutweave/timeline";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import type { engineStore } from "../engine/engineStore.ts";
import type { docStore } from "../state/docStore.ts";
import { isActive, type jobStore } from "../state/jobStore.ts";
import { HttpThumbnailSource } from "./httpThumbnails.ts";
import { MediaHttp } from "./mediaHttp.ts";
import { placeholderThumbnails, setThumbnailSource, type ThumbnailSource } from "./thumbnails.ts";
import { type HttpLoaderOptions, httpMediaLoaders, type LoaderMedia } from "./timelineLoaders.ts";

/** No loaders: the timeline draws its placeholders. */
export const NO_LOADERS: MediaLoaders = Object.freeze({});

export interface MediaLoadersState {
  /** What the timeline loads filmstrips and waveforms with; a new object asks again. */
  readonly loaders: MediaLoaders;
}

export const mediaLoadersStore = createStore<MediaLoadersState>(() => ({ loaders: NO_LOADERS }));

export function useMediaLoaders(): MediaLoaders {
  return useStore(mediaLoadersStore, (s) => s.loaders);
}

/** The slice of `window.cutweave.engine` used here. */
export interface MediaBridge {
  getConnection(): Promise<{
    readonly token: string;
    readonly httpUrl?: string;
    readonly port?: number;
  } | null>;
}

export interface MediaWiringDeps {
  readonly bridge: MediaBridge;
  readonly engine: typeof engineStore;
  readonly doc: typeof docStore;
  readonly jobs: typeof jobStore;
  readonly loaders?: typeof mediaLoadersStore;
  readonly setSource?: (source: ThumbnailSource) => void;
  readonly fetch?: typeof fetch;
  readonly decode?: HttpLoaderOptions["decode"];
  readonly crop?: HttpLoaderOptions["crop"];
}

export interface MediaWiring {
  /** Whether the real sources are installed. */
  readonly installed: boolean;
  stop(): void;
}

const ANALYSIS_LABELS = new Set(["thumbnails", "waveform"]);
/** The jobs a card's poster waits for (the frame index, then the poster itself). */
const POSTER_LABELS = new Set(["index", "thumbnails"]);

/** The loaders' view of an asset of the open document (and of its analysis jobs). */
export function loaderMediaOf(
  doc: typeof docStore,
  jobs: typeof jobStore,
  id: MediaId,
): (LoaderMedia & { readonly kind: MediaKind }) | null {
  const asset = doc.getState().project?.media[id];
  if (asset === undefined) return null;
  const stream = asset.probe.streams?.find((s) => s.kind === "audio");
  const making = Object.values(jobs.getState().jobs).some(
    (job) => job.media_id === id && job.label === "thumbnails" && isActive(job),
  );
  return {
    kind: asset.kind,
    hash: asset.content_hash ?? null,
    sampleRate: asset.probe.audio?.rate ?? stream?.sample_rate ?? null,
    makingFilmstrip: making,
  };
}

export function startMediaHttp(deps: MediaWiringDeps): MediaWiring {
  const loadersStore = deps.loaders ?? mediaLoadersStore;
  const setSource = deps.setSource ?? setThumbnailSource;
  const http = new MediaHttp({
    connection: async () => {
      const info = await deps.bridge.getConnection();
      if (info === null) return null;
      return { httpUrl: info.httpUrl ?? `http://127.0.0.1:${info.port ?? 0}`, token: info.token };
    },
    ...(deps.fetch && { fetch: deps.fetch }),
  });
  let installed = false;
  let check = 0;
  let stopped = false;

  /** A fresh wrapper, so the cards (which select the source) ask again. */
  const publish = () => {
    if (installed) setSource({ thumbnailFor: (request) => thumbs.thumbnailFor(request) });
  };
  const thumbs = new HttpThumbnailSource({
    http,
    contentHash: (id) => deps.doc.getState().project?.media[id]?.content_hash ?? null,
    onChange: publish,
    analysing: (id) =>
      Object.values(deps.jobs.getState().jobs).some(
        (job) => job.media_id === id && POSTER_LABELS.has(job.label) && isActive(job),
      ),
  });
  const loaders = httpMediaLoaders({
    http,
    media: (id) => loaderMediaOf(deps.doc, deps.jobs, id),
    ...(deps.decode && { decode: deps.decode }),
    ...(deps.crop && { crop: deps.crop }),
  });
  const handOut = () => loadersStore.setState({ loaders: { ...loaders } });

  function install(): void {
    installed = true;
    thumbs.reset();
    handOut();
  }

  function uninstall(): void {
    const was = installed;
    installed = false;
    if (was || stopped) {
      setSource(placeholderThumbnails);
      loadersStore.setState({ loaders: NO_LOADERS });
    }
  }

  async function checkEngine(): Promise<void> {
    const mine = ++check;
    const rpc = deps.engine.getState().rpc;
    if (rpc === null) return;
    let available = false;
    try {
      available = (await rpc.call("media.capabilities", {})).available;
    } catch {
      available = false;
    }
    if (mine !== check || stopped) return;
    if (available) install();
    else uninstall();
  }

  const offEngine = deps.engine.subscribe((state, previous) => {
    if (state.connection === "ready" && previous.connection !== "ready") void checkEngine();
  });
  if (deps.engine.getState().connection === "ready") void checkEngine();

  const offJobs = deps.jobs.subscribe((state, previous) => {
    if (!installed) return;
    const finishedNow = (labels: ReadonlySet<string>) =>
      Object.values(state.jobs).some((job) => {
        if (!labels.has(job.label) || isActive(job)) return false;
        const before = previous.jobs[job.id];
        return before === undefined || isActive(before);
      });
    if (finishedNow(ANALYSIS_LABELS)) {
      loaders.refresh();
      handOut();
    }
    // A card waiting for its poster asks again once the analysis is done.
    if (finishedNow(POSTER_LABELS)) publish();
  });

  return {
    get installed() {
      return installed;
    },
    stop() {
      stopped = true;
      check += 1;
      offEngine();
      offJobs();
      uninstall();
      loaders.dispose();
    },
  };
}
