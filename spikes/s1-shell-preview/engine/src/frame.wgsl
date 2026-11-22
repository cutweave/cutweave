// Spike S1 synthetic frame: scrolling hue gradient + checker, a white vertical bar moving
// 8 px/frame (judder/tearing indicator), a yellow horizontal bar, and a 7-segment readout of
// the frame number and the pts in milliseconds (the "pts-stamped glyph" for 0C.02 scrubbing).
// Distances are in 1080-line units, scaled to the output size.
// Also: an RGBA -> NV12 (BT.709 limited range) compute pass.

struct Uniforms {
  frame: u32,
  width: u32,
  height: u32,
  pts_ms: u32,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
  var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: VsOut;
  o.pos = vec4<f32>(p[vi], 0.0, 1.0);
  return o;
}

fn hsv2rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
  let k = vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0);
  let p = abs(fract(vec3<f32>(h) + k) * 6.0 - vec3<f32>(3.0));
  return v * mix(vec3<f32>(1.0), clamp(p - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0)), s);
}

fn in_rect(p: vec2<f32>, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
  return p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1;
}

// p in cell units: x in [0, 1], y in [0, 2] (top to bottom).
fn seven_seg(digit: u32, p: vec2<f32>) -> bool {
  var segs = array<u32, 10>(0x3Fu, 0x06u, 0x5Bu, 0x4Fu, 0x66u, 0x6Du, 0x7Du, 0x07u, 0x7Fu, 0x6Fu);
  let m = segs[digit % 10u];
  let t = 0.2;
  var on = false;
  if ((m & 1u) != 0u && in_rect(p, t, 0.0, 1.0 - t, t)) { on = true; }
  if ((m & 2u) != 0u && in_rect(p, 1.0 - t, t, 1.0, 1.0)) { on = true; }
  if ((m & 4u) != 0u && in_rect(p, 1.0 - t, 1.0, 1.0, 2.0 - t)) { on = true; }
  if ((m & 8u) != 0u && in_rect(p, t, 2.0 - t, 1.0 - t, 2.0)) { on = true; }
  if ((m & 16u) != 0u && in_rect(p, 0.0, 1.0, t, 2.0 - t)) { on = true; }
  if ((m & 32u) != 0u && in_rect(p, 0.0, t, t, 1.0)) { on = true; }
  if ((m & 64u) != 0u && in_rect(p, t, 1.0 - t * 0.5, 1.0 - t, 1.0 + t * 0.5)) { on = true; }
  return on;
}

// `value` as `n` zero-padded digits, top-left at `origin`, digit height `h_px` pixels.
fn number_lit(value: u32, n: u32, origin: vec2<f32>, h_px: f32, px: vec2<f32>) -> bool {
  let unit = h_px * 0.5;
  let advance = unit * 1.4;
  let local = px - origin;
  if (local.x < 0.0 || local.y < 0.0 || local.y > h_px) {
    return false;
  }
  let idx = u32(floor(local.x / advance));
  if (idx >= n) {
    return false;
  }
  let cell = vec2<f32>((local.x - f32(idx) * advance) / unit, local.y / unit);
  if (cell.x > 1.0) {
    return false;
  }
  var div = 1u;
  for (var i = 0u; i < n - 1u - idx; i += 1u) {
    div = div * 10u;
  }
  return seven_seg((value / div) % 10u, cell);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
  // The picture is authored on a 1080-line canvas and scaled to the output size (WP-0C.02
  // renders at the preview canvas size), so it looks the same at every resolution, like
  // footage would; low-res scrub previews are the same picture with fewer pixels.
  let scale = f32(u.height) / 1080.0;
  let px = in.pos.xy / scale;
  let w = f32(u.width) / scale;
  let vw = max(u32(round(w)), 1u);
  let f = f32(u.frame);
  let hue = fract((px.x + px.y * 0.5) / w - f / 240.0);
  var col = hsv2rgb(hue, 0.55, 0.6);
  let g = floor((px + vec2<f32>(f * 2.0, f)) / 60.0);
  if ((u32(g.x) + u32(g.y)) % 2u == 0u) {
    col = col * 0.8;
  }
  let bar_x = f32((u.frame * 8u) % vw);
  if (px.x >= bar_x && px.x < bar_x + 24.0) {
    col = vec3<f32>(1.0, 1.0, 1.0);
  }
  let bar_y = f32((u.frame * 4u) % 1080u);
  if (px.y >= bar_y && px.y < bar_y + 12.0) {
    col = vec3<f32>(1.0, 0.85, 0.2);
  }
  if (in_rect(px, 40.0, 40.0, 740.0, 290.0)) {
    col = vec3<f32>(0.04, 0.04, 0.04);
    if (number_lit(u.frame, 7u, vec2<f32>(70.0, 60.0), 120.0, px)) {
      col = vec3<f32>(1.0, 1.0, 1.0);
    }
    if (number_lit(u.pts_ms, 8u, vec2<f32>(70.0, 205.0), 60.0, px)) {
      col = vec3<f32>(0.3, 1.0, 0.4);
    }
  }
  return vec4<f32>(col, 1.0);
}

// ---- RGBA8 -> NV12 -------------------------------------------------------------------------

@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn luma(c: vec3<f32>) -> f32 {
  return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn to_byte(v: f32) -> u32 {
  return u32(clamp(round(v), 0.0, 255.0));
}

// One invocation packs 4 bytes: 4 luma samples (rows 0..h) or 2 interleaved CbCr pairs
// (rows h..h + h/2). Requires width % 4 == 0 and even height.
@compute @workgroup_size(64, 1, 1)
fn cs_nv12(@builtin(global_invocation_id) gid: vec3<u32>) {
  let w = u.width;
  let h = u.height;
  let qx = gid.x;
  if (qx >= w / 4u) {
    return;
  }
  let row = gid.y;
  if (row < h) {
    var packed = 0u;
    for (var i = 0u; i < 4u; i += 1u) {
      let c = textureLoad(src, vec2<u32>(qx * 4u + i, row), 0).rgb;
      packed = packed | (to_byte(16.0 + 219.0 * luma(c)) << (8u * i));
    }
    dst[(row * w) / 4u + qx] = packed;
  } else if (row < h + h / 2u) {
    let cy = row - h;
    var packed = 0u;
    for (var s = 0u; s < 2u; s += 1u) {
      let x0 = qx * 4u + s * 2u;
      let y0 = cy * 2u;
      let c = (textureLoad(src, vec2<u32>(x0, y0), 0).rgb
        + textureLoad(src, vec2<u32>(x0 + 1u, y0), 0).rgb
        + textureLoad(src, vec2<u32>(x0, y0 + 1u), 0).rgb
        + textureLoad(src, vec2<u32>(x0 + 1u, y0 + 1u), 0).rgb) * 0.25;
      let y = luma(c);
      let cb = (c.b - y) / 1.8556;
      let cr = (c.r - y) / 1.5748;
      packed = packed | (to_byte(128.0 + 224.0 * cb) << (16u * s));
      packed = packed | (to_byte(128.0 + 224.0 * cr) << (16u * s + 8u));
    }
    dst[(h * w + cy * w) / 4u + qx] = packed;
  }
}
