// SPDX-License-Identifier: GPL-3.0-or-later
// Preview presenters (WP-0C.02, arch §10 step 3).
//
// WebGPU, with the upload path switchable at runtime:
//   writeTexture  `queue.writeTexture` into an rgba8unorm texture (RGBA8 frames), or into
//                 r8unorm Y + rg8unorm UV textures with a BT.709 YUV->RGB shader (NV12 frames);
//   videoFrame    `new VideoFrame(pixels, {format: "RGBA" | "NV12"})` and
//                 `device.importExternalTexture` (Chromium does the upload and colour conversion).
// The canvas backing store is owned by the stage (stage.ts) and follows the canvas's device
// pixel size; frames of any size (low-res scrub previews, frames rendered before a resize was
// renegotiated) are scaled to it. `redraw()` repaints the last frame so a resized canvas never
// shows an empty frame. Canvas2D (`putImageData`, RGBA8 only) remains as a fallback.

import { FLICKS_PER_SECOND, FORMAT_NV12, FORMAT_RGBA8, type Frame } from "./types.ts";

// WebGPU flag values (GPUTextureUsage), not declared in TS 7 lib.dom.
const COPY_DST = 0x02;
const TEXTURE_BINDING = 0x04;

export type UploadPath = "writeTexture" | "videoFrame";
export const UPLOAD_PATHS: readonly UploadPath[] = ["writeTexture", "videoFrame"];

export interface Presenter {
  readonly kind: "webgpu" | "canvas2d";
  readonly adapter: string;
  /** Upload path used from the next frame on (Canvas2D ignores it). */
  upload: UploadPath;
  /** Path the last frame actually took, e.g. "writeTexture+yuv-shader nv12". */
  readonly lastPath: string;
  /** Called with (queue.submit -> onSubmittedWorkDone) in ms for every drawn frame. */
  onGpuDone: ((ms: number) => void) | null;
  /** Upload `frame` and draw it scaled to the canvas's current backing size. */
  draw(frame: Frame): void;
  /** Draw the last frame again (after the canvas was resized). False if there is none. */
  redraw(): boolean;
}

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

// BT.709 limited range -> RGB (matches the engine's cs_nv12).
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

const FS_EXTERNAL = /* wgsl */ `
@group(0) @binding(1) var tex: texture_external;
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
  return vec4<f32>(textureSampleBaseClampToEdge(tex, samp, in.uv).rgb, 1.0);
}`;

/** Textures for one frame size; created per format on first use. */
interface TexSet {
  key: string;
  rgba?: GPUTexture;
  y?: GPUTexture;
  uv?: GPUTexture;
  bgRgba?: GPUBindGroup;
  bgNv12?: GPUBindGroup;
}

/** Sizes kept alive at once: full size, low-res scrub size, and the size before a resize. */
const MAX_TEX_SETS = 3;

const NV12_COLOR: VideoColorSpaceInit = {
  primaries: "bt709",
  transfer: "bt709",
  matrix: "bt709",
  fullRange: false,
};
const RGBA_COLOR: VideoColorSpaceInit = {
  primaries: "bt709",
  transfer: "iec61966-2-1",
  matrix: "rgb",
  fullRange: true,
};

class WebGpuPresenter implements Presenter {
  readonly kind = "webgpu" as const;
  upload: UploadPath = "writeTexture";
  lastPath = "";
  onGpuDone: ((ms: number) => void) | null = null;
  private sets: TexSet[] = [];
  /** What `redraw()` repaints: a bind group over persistent textures, or the open VideoFrame. */
  private last: { pipe: GPURenderPipeline; bg: GPUBindGroup } | null = null;
  private lastFrame: VideoFrame | null = null;

  constructor(
    readonly adapter: string,
    private readonly device: GPUDevice,
    private readonly ctx: GPUCanvasContext,
    private readonly canvas: HTMLCanvasElement,
    private readonly pipeRgba: GPURenderPipeline,
    private readonly pipeNv12: GPURenderPipeline,
    private readonly pipeExternal: GPURenderPipeline,
    private readonly sampler: GPUSampler,
  ) {}

  private texSet(w: number, h: number): TexSet {
    const key = `${w}x${h}`;
    const i = this.sets.findIndex((s) => s.key === key);
    if (i >= 0) {
      const hit = this.sets[i] as TexSet;
      if (i > 0) this.sets.unshift(...this.sets.splice(i, 1)); // most recently used first
      return hit;
    }
    const set: TexSet = { key };
    this.sets.unshift(set);
    for (const old of this.sets.splice(MAX_TEX_SETS)) {
      for (const t of [old.rgba, old.y, old.uv]) t?.destroy();
    }
    return set;
  }

  private tex(w: number, h: number, format: GPUTextureFormat): GPUTexture {
    return this.device.createTexture({ size: [w, h], format, usage: TEXTURE_BINDING | COPY_DST });
  }

  draw(f: Frame): void {
    if (this.upload === "videoFrame" && typeof VideoFrame === "function") {
      const nv12 = f.format === FORMAT_NV12;
      const vf = new VideoFrame(f.pixels, {
        format: nv12 ? "NV12" : "RGBA",
        codedWidth: f.width,
        codedHeight: f.height,
        timestamp: Math.round((f.pts / FLICKS_PER_SECOND) * 1e6),
        layout: nv12
          ? [
              { offset: 0, stride: f.stride },
              { offset: f.stride * f.height, stride: f.stride },
            ]
          : [{ offset: 0, stride: f.stride }],
        colorSpace: nv12 ? NV12_COLOR : RGBA_COLOR,
      });
      this.lastFrame?.close();
      this.lastFrame = vf;
      this.last = null;
      this.lastPath = `videoFrame+importExternalTexture ${nv12 ? "nv12" : "rgba8"}`;
      this.drawExternal(vf);
      return;
    }
    this.lastFrame?.close();
    this.lastFrame = null;
    const q = this.device.queue;
    const set = this.texSet(f.width, f.height);
    if (f.format === FORMAT_RGBA8) {
      set.rgba ??= this.tex(f.width, f.height, "rgba8unorm");
      set.bgRgba ??= this.device.createBindGroup({
        layout: this.pipeRgba.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: this.sampler },
          { binding: 1, resource: set.rgba.createView() },
        ],
      });
      q.writeTexture(
        { texture: set.rgba },
        f.pixels,
        { bytesPerRow: f.stride, rowsPerImage: f.height },
        [f.width, f.height],
      );
      this.last = { pipe: this.pipeRgba, bg: set.bgRgba };
      this.lastPath = "writeTexture rgba8";
    } else if (f.format === FORMAT_NV12) {
      set.y ??= this.tex(f.width, f.height, "r8unorm");
      set.uv ??= this.tex(f.width / 2, f.height / 2, "rg8unorm");
      set.bgNv12 ??= this.device.createBindGroup({
        layout: this.pipeNv12.getBindGroupLayout(0),
        entries: [
          { binding: 0, resource: this.sampler },
          { binding: 1, resource: set.y.createView() },
          { binding: 2, resource: set.uv.createView() },
        ],
      });
      q.writeTexture({ texture: set.y }, f.pixels, { bytesPerRow: f.stride }, [f.width, f.height]);
      q.writeTexture(
        { texture: set.uv },
        f.pixels,
        { offset: f.stride * f.height, bytesPerRow: f.stride },
        [f.width / 2, f.height / 2],
      );
      this.last = { pipe: this.pipeNv12, bg: set.bgNv12 };
      this.lastPath = "writeTexture+yuv-shader nv12";
    } else {
      return;
    }
    this.submit(this.last.pipe, this.last.bg);
  }

  private drawExternal(vf: VideoFrame): void {
    const ext = this.device.importExternalTexture({ source: vf });
    const bg = this.device.createBindGroup({
      layout: this.pipeExternal.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: this.sampler },
        { binding: 1, resource: ext },
      ],
    });
    this.submit(this.pipeExternal, bg);
  }

  redraw(): boolean {
    if (this.lastFrame) {
      this.drawExternal(this.lastFrame);
      return true;
    }
    if (!this.last) return false;
    this.submit(this.last.pipe, this.last.bg);
    return true;
  }

  private submit(pipe: GPURenderPipeline, bg: GPUBindGroup): void {
    if (this.canvas.width === 0 || this.canvas.height === 0) return;
    const enc = this.device.createCommandEncoder();
    const pass = enc.beginRenderPass({
      colorAttachments: [
        {
          view: this.ctx.getCurrentTexture().createView(),
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
    this.device.queue.submit([enc.finish()]);
    const cb = this.onGpuDone;
    if (cb) {
      const t = performance.now();
      void this.device.queue.onSubmittedWorkDone().then(() => cb(performance.now() - t));
    }
  }
}

class Canvas2dPresenter implements Presenter {
  readonly kind = "canvas2d" as const;
  readonly adapter = "n/a";
  upload: UploadPath = "writeTexture";
  lastPath = "putImageData rgba8";
  onGpuDone: ((ms: number) => void) | null = null;
  private staging: OffscreenCanvas | null = null;
  private stagingCtx: OffscreenCanvasRenderingContext2D | null = null;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly ctx: CanvasRenderingContext2D,
  ) {}

  draw(f: Frame): void {
    if (f.format !== FORMAT_RGBA8) return; // NV12 needs the WebGPU path
    if (!this.staging || this.staging.width !== f.width || this.staging.height !== f.height) {
      this.staging = new OffscreenCanvas(f.width, f.height);
      this.stagingCtx = this.staging.getContext("2d");
    }
    this.stagingCtx?.putImageData(
      new ImageData(new Uint8ClampedArray(f.pixels, 0, f.width * f.height * 4), f.width, f.height),
      0,
      0,
    );
    this.redraw();
  }

  redraw(): boolean {
    if (!this.staging) return false;
    this.ctx.drawImage(this.staging, 0, 0, this.canvas.width, this.canvas.height);
    return true;
  }
}

export async function createPresenter(
  canvas: HTMLCanvasElement,
  preferGpu = true,
): Promise<Presenter> {
  if (preferGpu && "gpu" in navigator) {
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
    if (adapter) {
      const device = await adapter.requestDevice();
      // lib.dom (TS 7) lacks the "webgpu" getContext overload.
      const ctx = canvas.getContext("webgpu") as GPUCanvasContext | null;
      if (ctx) {
        const format = navigator.gpu.getPreferredCanvasFormat();
        ctx.configure({ device, format, alphaMode: "opaque" });
        const pipe = (fs: string) => {
          const module = device.createShaderModule({ code: VS + fs });
          return device.createRenderPipeline({
            layout: "auto",
            vertex: { module, entryPoint: "vs" },
            fragment: { module, entryPoint: "fs", targets: [{ format }] },
            primitive: { topology: "triangle-list" },
          });
        };
        const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
        const i = adapter.info;
        const name =
          [i.vendor, i.architecture, i.description].filter(Boolean).join(" / ") || "unknown";
        return new WebGpuPresenter(
          name,
          device,
          ctx,
          canvas,
          pipe(FS_RGBA),
          pipe(FS_NV12),
          pipe(FS_EXTERNAL),
          sampler,
        );
      }
    }
  }
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!ctx) throw new Error("no canvas context");
  return new Canvas2dPresenter(canvas, ctx);
}
