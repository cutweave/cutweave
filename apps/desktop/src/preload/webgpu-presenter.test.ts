// SPDX-License-Identifier: GPL-3.0-or-later
// The WebGPU presenter's upload and draw logic (WP-1M.04) against a recording fake device: the
// S1 upload path (writeTexture, RGBA8 and NV12 with padded rows), texture reuse per frame size,
// redraw after a resize, and failure reporting. The real GPU path runs in the Playwright e2e.
import { describe, expect, it } from "vitest";
import type { PreviewFrame } from "../shared/api.ts";
import {
  createWebGpuPresenter,
  type GpuLike,
  MAX_TEXTURE_SETS,
  PresenterError,
  WebGpuPresenter,
} from "./webgpu-presenter.ts";

interface Call {
  readonly op: string;
  readonly args: unknown[];
}

function fakeGpu() {
  const calls: Call[] = [];
  const log = (op: string, ...args: unknown[]) => calls.push({ op, args });
  let textures = 0;
  let lost: (info: { reason: string; message: string }) => void = () => {};
  const device = {
    lost: new Promise((r) => {
      lost = r;
    }),
    queue: {
      writeTexture: (...args: unknown[]) => log("writeTexture", ...args),
      submit: (cmds: unknown[]) => log("submit", cmds.length),
    },
    createTexture: (desc: { size: number[]; format: string }) => {
      const id = ++textures;
      log("createTexture", desc.format, desc.size);
      return {
        id,
        format: desc.format,
        createView: () => ({ view: id }),
        destroy: () => log("destroyTexture", id),
      };
    },
    createBindGroup: (desc: unknown) => {
      log("createBindGroup");
      return { desc };
    },
    createCommandEncoder: () => ({
      beginRenderPass: () => ({
        setPipeline: (p: { name: string }) => log("setPipeline", p.name),
        setBindGroup: () => {},
        draw: (n: number) => log("draw", n),
        end: () => {},
      }),
      finish: () => ({}),
    }),
    destroy: () => log("destroyDevice"),
  };
  const canvas = { width: 640, height: 360 };
  const context = {
    getCurrentTexture: () => ({ createView: () => ({}) }),
    unconfigure: () => log("unconfigure"),
  };
  const pipe = (name: string) => ({ name, getBindGroupLayout: () => ({}) });
  const presenter = new WebGpuPresenter({
    adapter: "Fake GPU",
    device: device as unknown as GPUDevice,
    context: context as unknown as GPUCanvasContext,
    canvas,
    pipeRgba: pipe("rgba") as unknown as GPURenderPipeline,
    pipeNv12: pipe("nv12") as unknown as GPURenderPipeline,
    sampler: {} as GPUSampler,
  });
  return {
    presenter,
    calls,
    canvas,
    ops: () => calls.map((c) => c.op),
    lose: (message: string) => lost({ reason: "unknown", message }),
  };
}

function frame(over: Partial<PreviewFrame> = {}): PreviewFrame {
  return {
    pts: 0,
    duration: 0,
    seq: 1,
    width: 64,
    height: 36,
    format: "rgba8",
    bytes: new ArrayBuffer(80 * 36),
    stride: 80 * 4,
    plane1Offset: 0,
    plane1Stride: 0,
    lowres: false,
    rev: 1,
    configGen: 1,
    renderAgeMs: 0,
    publishAgeMs: 0,
    ...over,
  };
}

describe("WebGpuPresenter", () => {
  it("uploads RGBA8 rows with the ring's stride and draws a fullscreen triangle", () => {
    const g = fakeGpu();
    const f = frame();
    g.presenter.present(f);
    const write = g.calls.find((c) => c.op === "writeTexture");
    expect(write?.args[1]).toBe(f.bytes);
    expect(write?.args[2]).toEqual({ offset: 0, bytesPerRow: 320, rowsPerImage: 36 });
    expect(write?.args[3]).toEqual([64, 36]);
    expect(g.ops()).toEqual([
      "createTexture",
      "createBindGroup",
      "writeTexture",
      "setPipeline",
      "draw",
      "submit",
    ]);
    expect(g.calls.find((c) => c.op === "createTexture")?.args).toEqual(["rgba8unorm", [64, 36]]);
    expect(g.presenter.stats()).toMatchObject({
      kind: "webgpu",
      adapter: "Fake GPU",
      lastPath: "writeTexture rgba8",
      uploads: 1,
      draws: 1,
      errors: 0,
      lost: false,
    });
  });

  it("uploads NV12 as a Y plane and a half-size UV plane at plane1Offset", () => {
    const g = fakeGpu();
    const f = frame({ format: "nv12", stride: 128, plane1Offset: 128 * 36, plane1Stride: 128 });
    g.presenter.present(f);
    const writes = g.calls.filter((c) => c.op === "writeTexture");
    expect(writes.map((w) => [w.args[2], w.args[3]])).toEqual([
      [{ offset: 0, bytesPerRow: 128, rowsPerImage: 36 }, [64, 36]],
      [{ offset: 128 * 36, bytesPerRow: 128, rowsPerImage: 18 }, [32, 18]],
    ]);
    const made = g.calls.filter((c) => c.op === "createTexture").map((c) => c.args);
    expect(made).toEqual([
      ["r8unorm", [64, 36]],
      ["rg8unorm", [32, 18]],
    ]);
    expect(g.calls.find((c) => c.op === "setPipeline")?.args).toEqual(["nv12"]);
  });

  it("reuses textures for the same size and keeps at most three sizes", () => {
    const g = fakeGpu();
    g.presenter.present(frame());
    g.presenter.present(frame({ seq: 2 }));
    expect(g.calls.filter((c) => c.op === "createTexture")).toHaveLength(1);
    for (let i = 1; i <= MAX_TEXTURE_SETS; i++) {
      g.presenter.present(frame({ width: 64 + 2 * i, stride: (64 + 2 * i) * 4 }));
    }
    expect(g.calls.filter((c) => c.op === "destroyTexture")).toHaveLength(1);
  });

  it("redraw() repaints the last frame without uploading; nothing to repaint before a frame", () => {
    const g = fakeGpu();
    expect(g.presenter.redraw()).toBe(false);
    g.presenter.present(frame());
    expect(g.presenter.redraw()).toBe(true);
    expect(g.calls.filter((c) => c.op === "writeTexture")).toHaveLength(1);
    expect(g.calls.filter((c) => c.op === "submit")).toHaveLength(2);
    g.canvas.width = 0;
    expect(g.presenter.redraw()).toBe(false);
  });

  it("counts a failed upload instead of throwing into the present loop", () => {
    const p = fakeGpu();
    // A frame whose buffer went away under the upload (the fake device accepts anything else).
    p.presenter.present({
      ...frame(),
      get bytes(): ArrayBuffer {
        throw new Error("detached");
      },
    });
    expect(p.presenter.stats()).toMatchObject({ errors: 1, lastError: "detached" });
  });

  it("stops drawing once the device is lost and reports it", async () => {
    const g = fakeGpu();
    g.lose("gone");
    await Promise.resolve();
    await Promise.resolve();
    g.presenter.present(frame());
    expect(g.calls.filter((c) => c.op === "submit")).toHaveLength(0);
    expect(g.presenter.stats()).toMatchObject({ lost: true, lastError: "device lost: gone" });
  });

  it("destroy() releases textures, the context and the device", () => {
    const g = fakeGpu();
    g.presenter.present(frame());
    g.presenter.destroy();
    expect(g.ops().slice(-3)).toEqual(["destroyTexture", "unconfigure", "destroyDevice"]);
    g.presenter.present(frame());
    expect(g.calls.filter((c) => c.op === "submit")).toHaveLength(1);
  });
});

describe("createWebGpuPresenter", () => {
  const canvas = { getContext: () => null } as unknown as HTMLCanvasElement;

  it("reports why WebGPU cannot present", async () => {
    await expect(createWebGpuPresenter(canvas, undefined)).rejects.toMatchObject({
      reason: "no-webgpu",
    });
    const noAdapter: GpuLike = {
      requestAdapter: async () => null,
      getPreferredCanvasFormat: () => "bgra8unorm",
    };
    await expect(createWebGpuPresenter(canvas, noAdapter)).rejects.toBeInstanceOf(PresenterError);
    const destroyed: string[] = [];
    const withDevice: GpuLike = {
      requestAdapter: async () =>
        ({
          requestDevice: async () => ({ destroy: () => destroyed.push("device") }),
        }) as unknown as GPUAdapter,
      getPreferredCanvasFormat: () => "bgra8unorm",
    };
    await expect(createWebGpuPresenter(canvas, withDevice)).rejects.toMatchObject({
      reason: "no-context",
    });
    expect(destroyed).toEqual(["device"]);
  });
});
