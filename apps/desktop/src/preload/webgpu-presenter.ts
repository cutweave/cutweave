// SPDX-License-Identifier: GPL-3.0-or-later
// The WebGPU frame presenter (WP-1M.04, arch §10 step 3), plugged into the preload's present loop
// (`setFramePresenter`). It runs in the preload's isolated world and draws into the page's
// preview canvas, which the page hands over through `window.cutweave.preview.attachCanvas`; the
// pixels never cross the context bridge (Spike S1 finding 2).
//
// Upload path: Spike S1's choice, `queue.writeTexture` straight from the ring frame's bytes:
//   - RGBA8 into an `rgba8unorm` texture;
//   - NV12 into an `r8unorm` Y texture and an `rg8unorm` UV texture, converted by a BT.709
//     limited-range shader (the engine's NV12 matrix).
// Rows may be padded (`stride`, `plane1Stride`); the UV plane starts at `plane1Offset`.
// A fullscreen triangle scales the frame to the canvas's backing size, so a quarter-size scrub
// frame or a frame rendered before a resize renegotiated still fills it. `redraw()` repaints the
// last frame after the page resized (and so cleared) the canvas. Textures are kept per frame
// size (at most 3: full size, the low-res scrub size and the size before a resize).

import type { PreviewFrame } from "../shared/api.ts";
import type { FramePresenter } from "./present-loop.ts";

// WebGPU flag values (`GPUTextureUsage`), not declared as globals in Node's types.
const COPY_DST = 0x02;
const TEXTURE_BINDING = 0x04;

/** Texture sets kept alive at once. */
export const MAX_TEXTURE_SETS = 3;

const VS = /* wgsl */ `
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
  var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var o: VsOut;
  o.pos = vec4<f32>(p[i], 0.0, 1.0);
  o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
  return o;
}
@group(0) @binding(0) var samp: sampler;
`;

const FS_RGBA = /* wgsl */ `
@group(0) @binding(1) var tex: texture_2d<f32>;
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
  return vec4<f32>(textureSample(tex, samp, in.uv).rgb, 1.0);
}`;

// BT.709 limited range -> RGB (matches the engine's NV12 conversion).
const FS_NV12 = /* wgsl */ `
@group(0) @binding(1) var tex_y: texture_2d<f32>;
@group(0) @binding(2) var tex_uv: texture_2d<f32>;
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
  let y = (textureSample(tex_y, samp, in.uv).r * 255.0 - 16.0) / 219.0;
  let c = (textureSample(tex_uv, samp, in.uv).rg * 255.0 - vec2<f32>(128.0)) / 224.0;
  let r = y + 1.5748 * c.y;
  let b = y + 1.8556 * c.x;
  let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
  return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}`;

/** What the page learns about the presenter (small plain values). */
export interface PresenterStats {
  readonly kind: "webgpu";
  readonly adapter: string;
  /** e.g. `writeTexture nv12`. */
  readonly lastPath: string;
  readonly uploads: number;
  readonly draws: number;
  readonly errors: number;
  readonly lastError: string | null;
  /** The GPU device was lost: nothing draws any more (fall back). */
  readonly lost: boolean;
}

/** A presenter the preload can attach to a canvas. */
export interface CanvasPresenter extends FramePresenter {
  readonly adapter: string;
  redraw(): boolean;
  destroy(): void;
  stats(): PresenterStats;
}

/** Why the presenter could not be created (the page falls back to JPEG frames). */
export class PresenterError extends Error {
  readonly reason: "no-webgpu" | "no-adapter" | "no-context" | "device-failed";
  constructor(reason: PresenterError["reason"], message: string) {
    super(message);
    this.name = "PresenterError";
    this.reason = reason;
  }
}

interface TexSet {
  readonly key: string;
  rgba?: GPUTexture;
  y?: GPUTexture;
  uv?: GPUTexture;
  bgRgba?: GPUBindGroup;
  bgNv12?: GPUBindGroup;
}

export interface PresenterParts {
  readonly adapter: string;
  readonly device: GPUDevice;
  readonly context: GPUCanvasContext;
  readonly canvas: { readonly width: number; readonly height: number };
  readonly pipeRgba: GPURenderPipeline;
  readonly pipeNv12: GPURenderPipeline;
  readonly sampler: GPUSampler;
}

const errorText = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export class WebGpuPresenter implements CanvasPresenter {
  readonly adapter: string;
  readonly #p: PresenterParts;
  #sets: TexSet[] = [];
  #last: { pipe: GPURenderPipeline; bg: GPUBindGroup } | null = null;
  #lastPath = "";
  #uploads = 0;
  #draws = 0;
  #errors = 0;
  #lastError: string | null = null;
  #lost = false;
  #destroyed = false;

  constructor(parts: PresenterParts) {
    this.#p = parts;
    this.adapter = parts.adapter;
    parts.device.lost?.then((info) => {
      this.#lost = true;
      if (!this.#destroyed) this.#lastError = `device lost: ${info.message || info.reason}`;
    });
  }

  present(frame: PreviewFrame): void {
    if (this.#lost || this.#destroyed) return;
    try {
      this.#upload(frame);
      this.redraw();
    } catch (e) {
      this.#errors += 1;
      this.#lastError = errorText(e);
    }
  }

  redraw(): boolean {
    if (this.#last === null || this.#lost || this.#destroyed) return false;
    const { canvas } = this.#p;
    if (canvas.width === 0 || canvas.height === 0) return false;
    this.#submit(this.#last.pipe, this.#last.bg);
    return true;
  }

  destroy(): void {
    if (this.#destroyed) return;
    this.#destroyed = true;
    for (const set of this.#sets) for (const t of [set.rgba, set.y, set.uv]) t?.destroy();
    this.#sets = [];
    this.#last = null;
    try {
      this.#p.context.unconfigure();
    } catch {
      // Already unconfigured (device lost).
    }
    this.#p.device.destroy();
  }

  stats(): PresenterStats {
    return {
      kind: "webgpu",
      adapter: this.adapter,
      lastPath: this.#lastPath,
      uploads: this.#uploads,
      draws: this.#draws,
      errors: this.#errors,
      lastError: this.#lastError,
      lost: this.#lost,
    };
  }

  #texSet(width: number, height: number): TexSet {
    const key = `${width}x${height}`;
    const i = this.#sets.findIndex((s) => s.key === key);
    if (i >= 0) {
      const hit = this.#sets[i] as TexSet;
      if (i > 0) this.#sets.unshift(...this.#sets.splice(i, 1)); // most recently used first
      return hit;
    }
    const set: TexSet = { key };
    this.#sets.unshift(set);
    for (const old of this.#sets.splice(MAX_TEXTURE_SETS)) {
      for (const t of [old.rgba, old.y, old.uv]) t?.destroy();
    }
    return set;
  }

  #texture(width: number, height: number, format: GPUTextureFormat): GPUTexture {
    return this.#p.device.createTexture({
      size: [width, height],
      format,
      usage: TEXTURE_BINDING | COPY_DST,
    });
  }

  #upload(f: PreviewFrame): void {
    const { device, sampler, pipeRgba, pipeNv12 } = this.#p;
    const queue = device.queue;
    const set = this.#texSet(f.width, f.height);
    if (f.format === "rgba8") {
      set.rgba ??= this.#texture(f.width, f.height, "rgba8unorm");
      set.bgRgba ??= device.createBindGroup({
        layout: pipeRgba.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: sampler },
          { binding: 1, resource: set.rgba.createView() },
        ],
      });
      queue.writeTexture(
        { texture: set.rgba },
        f.bytes,
        { offset: 0, bytesPerRow: f.stride, rowsPerImage: f.height },
        [f.width, f.height],
      );
      this.#last = { pipe: pipeRgba, bg: set.bgRgba };
      this.#lastPath = "writeTexture rgba8";
    } else {
      const cw = Math.ceil(f.width / 2);
      const ch = Math.ceil(f.height / 2);
      set.y ??= this.#texture(f.width, f.height, "r8unorm");
      set.uv ??= this.#texture(cw, ch, "rg8unorm");
      set.bgNv12 ??= device.createBindGroup({
        layout: pipeNv12.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: sampler },
          { binding: 1, resource: set.y.createView() },
          { binding: 2, resource: set.uv.createView() },
        ],
      });
      queue.writeTexture(
        { texture: set.y },
        f.bytes,
        { offset: 0, bytesPerRow: f.stride, rowsPerImage: f.height },
        [f.width, f.height],
      );
      queue.writeTexture(
        { texture: set.uv },
        f.bytes,
        { offset: f.plane1Offset, bytesPerRow: f.plane1Stride, rowsPerImage: ch },
        [cw, ch],
      );
      this.#last = { pipe: pipeNv12, bg: set.bgNv12 };
      this.#lastPath = "writeTexture+yuv-shader nv12";
    }
    this.#uploads += 1;
  }

  #submit(pipe: GPURenderPipeline, bg: GPUBindGroup): void {
    const { device, context } = this.#p;
    const encoder = device.createCommandEncoder();
    const pass = encoder.beginRenderPass({
      colorAttachments: [
        {
          view: context.getCurrentTexture().createView(),
          loadOp: "clear",
          storeOp: "store",
          clearValue: { r: 0, g: 0, b: 0, a: 1 },
        },
      ],
    });
    pass.setPipeline(pipe);
    pass.setBindGroup(0, bg);
    pass.draw(3);
    pass.end();
    device.queue.submit([encoder.finish()]);
    this.#draws += 1;
  }
}

/** The slice of `navigator.gpu` the presenter needs. */
export interface GpuLike {
  requestAdapter(options?: GPURequestAdapterOptions): Promise<GPUAdapter | null>;
  getPreferredCanvasFormat(): GPUTextureFormat;
}

/**
 * Creates the presenter on `canvas`: a high-performance adapter, a device, the canvas's
 * `webgpu` context and the two pipelines. Throws a {@link PresenterError} with the reason.
 */
export async function createWebGpuPresenter(
  canvas: HTMLCanvasElement,
  gpu: GpuLike | undefined = (globalThis.navigator as { gpu?: GpuLike } | undefined)?.gpu,
): Promise<WebGpuPresenter> {
  if (gpu === undefined) throw new PresenterError("no-webgpu", "WebGPU is not available");
  const adapter = await gpu.requestAdapter({ powerPreference: "high-performance" });
  if (adapter === null) throw new PresenterError("no-adapter", "no WebGPU adapter");
  let device: GPUDevice;
  try {
    device = await adapter.requestDevice();
  } catch (e) {
    throw new PresenterError("device-failed", errorText(e));
  }
  // lib.dom has no "webgpu" overload of getContext.
  const context = canvas.getContext("webgpu" as "2d") as unknown as GPUCanvasContext | null;
  if (context === null) {
    device.destroy();
    throw new PresenterError("no-context", "the canvas has no WebGPU context");
  }
  const format = gpu.getPreferredCanvasFormat();
  context.configure({ device, format, alphaMode: "opaque" });
  const pipe = (fs: string): GPURenderPipeline => {
    const module = device.createShaderModule({ code: VS + fs });
    return device.createRenderPipeline({
      layout: "auto",
      vertex: { module, entryPoint: "vs" },
      fragment: { module, entryPoint: "fs", targets: [{ format }] },
      primitive: { topology: "triangle-list" },
    });
  };
  const info = adapter.info;
  const name =
    [info?.vendor, info?.architecture, info?.description].filter(Boolean).join(" / ") || "unknown";
  return new WebGpuPresenter({
    adapter: name,
    device,
    context,
    canvas,
    pipeRgba: pipe(FS_RGBA),
    pipeNv12: pipe(FS_NV12),
    sampler: device.createSampler({ magFilter: "linear", minFilter: "linear" }),
  });
}
