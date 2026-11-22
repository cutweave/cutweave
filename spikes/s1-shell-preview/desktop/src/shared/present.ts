// SPDX-License-Identifier: GPL-3.0-or-later
// Minimal presenters for the spike: WebGPU `queue.writeTexture` (RGBA8 + NV12 via a YUV
// shader) with a Canvas2D fallback (RGBA8 only). The real canvas, overlays and the
// `importExternalTexture` path are WP-0C.02.

import { FORMAT_NV12, FORMAT_RGBA8, type Frame } from "./types.ts";

// WebGPU flag values (GPUTextureUsage / GPUShaderStage), not declared in TS 7 lib.dom.
const COPY_DST = 0x02;
const TEXTURE_BINDING = 0x04;
const FRAGMENT = 0x2;

export interface Presenter {
  readonly kind: "webgpu" | "canvas2d";
  readonly adapter: string;
  draw(frame: Frame): void;
}

const SHADER = /* wgsl */ `
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs(@builtin(vertex_index) i: u32) -> VsOut {
  var p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  var o: VsOut;
  o.pos = vec4<f32>(p[i], 0.0, 1.0);
  o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
  return o;
}

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var tex_rgba: texture_2d<f32>;
@group(0) @binding(2) var tex_y: texture_2d<f32>;
@group(0) @binding(3) var tex_uv: texture_2d<f32>;

@fragment fn fs_rgba(in: VsOut) -> @location(0) vec4<f32> {
  return vec4<f32>(textureSample(tex_rgba, samp, in.uv).rgb, 1.0);
}

// BT.709 limited range -> RGB.
@fragment fn fs_nv12(in: VsOut) -> @location(0) vec4<f32> {
  let y = (textureSample(tex_y, samp, in.uv).r * 255.0 - 16.0) / 219.0;
  let c = (textureSample(tex_uv, samp, in.uv).rg * 255.0 - vec2<f32>(128.0)) / 224.0;
  let r = y + 1.5748 * c.y;
  let b = y + 1.8556 * c.x;
  let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
  return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
`;

class WebGpuPresenter implements Presenter {
  readonly kind = "webgpu" as const;
  private rgba: GPUTexture | null = null;
  private y: GPUTexture | null = null;
  private uv: GPUTexture | null = null;
  private bindGroup: GPUBindGroup | null = null;
  private dims = "";

  constructor(
    readonly adapter: string,
    private readonly device: GPUDevice,
    private readonly ctx: GPUCanvasContext,
    private readonly canvas: HTMLCanvasElement,
    private readonly pipeRgba: GPURenderPipeline,
    private readonly pipeNv12: GPURenderPipeline,
    private readonly sampler: GPUSampler,
  ) {}

  private ensure(w: number, h: number): void {
    const key = `${w}x${h}`;
    if (key === this.dims) return;
    this.dims = key;
    this.canvas.width = w;
    this.canvas.height = h;
    for (const t of [this.rgba, this.y, this.uv]) t?.destroy();
    const usage = TEXTURE_BINDING | COPY_DST;
    this.rgba = this.device.createTexture({ size: [w, h], format: "rgba8unorm", usage });
    this.y = this.device.createTexture({ size: [w, h], format: "r8unorm", usage });
    this.uv = this.device.createTexture({ size: [w / 2, h / 2], format: "rg8unorm", usage });
    this.bindGroup = this.device.createBindGroup({
      layout: this.pipeRgba.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: this.sampler },
        { binding: 1, resource: this.rgba.createView() },
        { binding: 2, resource: this.y.createView() },
        { binding: 3, resource: this.uv.createView() },
      ],
    });
  }

  draw(f: Frame): void {
    this.ensure(f.width, f.height);
    const q = this.device.queue;
    if (f.format === FORMAT_RGBA8 && this.rgba) {
      q.writeTexture({ texture: this.rgba }, f.pixels, { bytesPerRow: f.stride, rowsPerImage: f.height }, [f.width, f.height]);
    } else if (f.format === FORMAT_NV12 && this.y && this.uv) {
      q.writeTexture({ texture: this.y }, f.pixels, { bytesPerRow: f.stride }, [f.width, f.height]);
      q.writeTexture(
        { texture: this.uv },
        f.pixels,
        { offset: f.stride * f.height, bytesPerRow: f.stride },
        [f.width / 2, f.height / 2],
      );
    } else {
      return;
    }
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
    pass.setPipeline(f.format === FORMAT_NV12 ? this.pipeNv12 : this.pipeRgba);
    if (this.bindGroup) pass.setBindGroup(0, this.bindGroup);
    pass.draw(3);
    pass.end();
    q.submit([enc.finish()]);
  }
}

class Canvas2dPresenter implements Presenter {
  readonly kind = "canvas2d" as const;
  readonly adapter = "n/a";
  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly ctx: CanvasRenderingContext2D,
  ) {}
  draw(f: Frame): void {
    if (f.format !== FORMAT_RGBA8) return; // NV12 needs the WebGPU path
    if (this.canvas.width !== f.width) this.canvas.width = f.width;
    if (this.canvas.height !== f.height) this.canvas.height = f.height;
    this.ctx.putImageData(new ImageData(new Uint8ClampedArray(f.pixels), f.width, f.height), 0, 0);
  }
}

export async function createPresenter(canvas: HTMLCanvasElement, preferGpu = true): Promise<Presenter> {
  if (preferGpu && "gpu" in navigator) {
    const adapter = await navigator.gpu.requestAdapter({ powerPreference: "high-performance" });
    if (adapter) {
      const device = await adapter.requestDevice();
      // lib.dom (TS 7) lacks the "webgpu" getContext overload.
      const ctx = canvas.getContext("webgpu") as GPUCanvasContext | null;
      if (ctx) {
        const format = navigator.gpu.getPreferredCanvasFormat();
        ctx.configure({ device, format, alphaMode: "opaque" });
        const module = device.createShaderModule({ code: SHADER });
        const layout = device.createBindGroupLayout({
          entries: [
            { binding: 0, visibility: FRAGMENT, sampler: {} },
            { binding: 1, visibility: FRAGMENT, texture: {} },
            { binding: 2, visibility: FRAGMENT, texture: {} },
            { binding: 3, visibility: FRAGMENT, texture: {} },
          ],
        });
        const pipelineLayout = device.createPipelineLayout({ bindGroupLayouts: [layout] });
        const pipe = (entryPoint: string) =>
          device.createRenderPipeline({
            layout: pipelineLayout,
            vertex: { module, entryPoint: "vs" },
            fragment: { module, entryPoint, targets: [{ format }] },
            primitive: { topology: "triangle-list" },
          });
        const sampler = device.createSampler({ magFilter: "linear", minFilter: "linear" });
        const i = adapter.info;
        const name = [i.vendor, i.architecture, i.description].filter(Boolean).join(" / ") || "unknown";
        return new WebGpuPresenter(name, device, ctx, canvas, pipe("fs_rgba"), pipe("fs_nv12"), sampler);
      }
    }
  }
  const ctx = canvas.getContext("2d", { alpha: false });
  if (!ctx) throw new Error("no canvas context");
  return new Canvas2dPresenter(canvas, ctx);
}
