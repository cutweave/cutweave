// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1I.03: the editor installs the real thumbnail source and timeline loaders only when the
// connected engine has media, and hands the timeline fresh loaders when an analysis finishes.
import type { JobInfo, MediaId, RpcClient } from "@cutweave/protocol";
import { describe, expect, it, vi } from "vitest";
import { createStore } from "zustand/vanilla";
import { createEngineStore } from "../engine/engineStore.ts";
import { createDocStore } from "../state/docStore.ts";
import { createJobStore } from "../state/jobStore.ts";
import {
  loaderMediaOf,
  type MediaLoadersState,
  NO_LOADERS,
  startMediaHttp,
} from "./mediaWiring.ts";
import { placeholderThumbnails, type ThumbnailSource } from "./thumbnails.ts";

const flush = () => new Promise<void>((r) => setTimeout(r, 0));

function setup(available: boolean) {
  const engine = createEngineStore();
  const jobs = createJobStore();
  const loaders = createStore<MediaLoadersState>(() => ({ loaders: NO_LOADERS }));
  const sources: ThumbnailSource[] = [];
  const call = vi.fn(async (method: string) => {
    if (method !== "media.capabilities") throw new Error(method);
    return { available, codecs: [], hw_encode: [], hw_decode: [] };
  });
  const wiring = startMediaHttp({
    bridge: { getConnection: async () => ({ token: "t", port: 1 }) },
    engine,
    doc: createDocStore(),
    jobs,
    loaders,
    setSource: (s) => sources.push(s),
    fetch: (async () => new Response("{}", { status: 500 })) as typeof fetch,
  });
  const connect = () =>
    engine.setState({ rpc: { call } as unknown as RpcClient, connection: "ready" });
  return { engine, jobs, loaders, sources, call, wiring, connect };
}

const job = (id: string, label: string, state: JobInfo["state"]): JobInfo =>
  ({
    id,
    kind: "analysis",
    label,
    media_id: "med_1",
    state,
    progress: { fraction: null, message: null },
    cancel_requested: false,
    error: null,
  }) as unknown as JobInfo;

describe("loaderMediaOf", () => {
  it("says when the import analysis is making an asset's filmstrip", () => {
    const doc = createDocStore();
    doc.setState({
      project: {
        media: {
          med_1: {
            kind: "video",
            content_hash: "blake3-fast:ab",
            probe: { streams: [{ kind: "audio", sample_rate: 44_100 }], audio: null },
          },
        },
      },
    } as never);
    const jobs = createJobStore();
    const id = "med_1" as MediaId;
    expect(loaderMediaOf(doc, jobs, id)).toEqual({
      kind: "video",
      hash: "blake3-fast:ab",
      sampleRate: 44_100,
      makingFilmstrip: false,
    });
    jobs.getState().applyProgress(job("job_1", "thumbnails", "running"));
    expect(loaderMediaOf(doc, jobs, id)?.makingFilmstrip).toBe(true);
    jobs.getState().applyProgress(job("job_1", "thumbnails", "succeeded"));
    expect(loaderMediaOf(doc, jobs, id)?.makingFilmstrip).toBe(false);
    expect(loaderMediaOf(doc, jobs, "med_2" as MediaId)).toBeNull();
  });
});

describe("startMediaHttp", () => {
  it("installs the thumbnail source and the timeline loaders when the engine has media", async () => {
    const s = setup(true);
    expect(s.wiring.installed).toBe(false);
    s.connect();
    await flush();
    expect(s.call).toHaveBeenCalledWith("media.capabilities", {});
    expect(s.wiring.installed).toBe(true);
    expect(s.sources.length).toBeGreaterThan(0);
    expect(s.sources.at(-1)).not.toBe(placeholderThumbnails);
    const loaders = s.loaders.getState().loaders;
    expect(typeof loaders.filmstripTile).toBe("function");
    expect(typeof loaders.waveform).toBe("function");

    // A finished analysis: fresh loaders (the timeline asks again); others change nothing.
    s.jobs.getState().applyProgress(job("job_1", "thumbnails", "running"));
    expect(s.loaders.getState().loaders).toBe(loaders);
    s.jobs.getState().applyProgress(job("job_1", "thumbnails", "succeeded"));
    const fresh = s.loaders.getState().loaders;
    expect(fresh).not.toBe(loaders);
    s.jobs.getState().applyProgress(job("job_2", "index", "succeeded"));
    expect(s.loaders.getState().loaders).toBe(fresh);
    // A failed waveform ends too: ask again (the route says what is missing).
    s.jobs.getState().applyProgress(job("job_3", "waveform", "running"));
    s.jobs.getState().applyProgress(job("job_3", "waveform", "failed"));
    expect(s.loaders.getState().loaders).not.toBe(fresh);

    s.wiring.stop();
    expect(s.sources.at(-1)).toBe(placeholderThumbnails);
    expect(s.loaders.getState().loaders).toBe(NO_LOADERS);
  });

  it("hands the cards a fresh source when a poster's analysis ends, so a waiting card asks again", async () => {
    const s = setup(true);
    s.connect();
    await flush();
    const before = s.sources.length;
    s.jobs.getState().applyProgress(job("job_1", "index", "running"));
    expect(s.sources.length).toBe(before);
    s.jobs.getState().applyProgress(job("job_1", "index", "succeeded"));
    expect(s.sources.length).toBe(before + 1);
    s.jobs.getState().applyProgress(job("job_2", "waveform", "succeeded"));
    expect(s.sources.length).toBe(before + 1);
    s.wiring.stop();
  });

  it("keeps the placeholders when the engine has no media backend", async () => {
    const s = setup(false);
    s.connect();
    await flush();
    expect(s.wiring.installed).toBe(false);
    expect(s.sources).toEqual([]);
    expect(s.loaders.getState().loaders).toBe(NO_LOADERS);
    s.wiring.stop();
  });

  it("asks again after a reconnect", async () => {
    const s = setup(true);
    s.connect();
    await flush();
    s.engine.setState({ connection: "reconnecting" });
    s.connect();
    await flush();
    expect(s.call).toHaveBeenCalledTimes(2);
    s.wiring.stop();
  });
});
