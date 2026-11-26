// SPDX-License-Identifier: GPL-3.0-or-later
// Places one layer over the canvas, in place: a quad over the layer's output-pixel bounds; the
// fragment maps its pixel centre back to the layer (inverse affine), computes 1/2-px edge
// coverage, samples the decoded texture with exact-weight bilinear / trilinear filtering
// (textureLoad, never a hardware sampler: S4 finding 1) and outputs premultiplied colour scaled
// by coverage x opacity. Fixed-function blending does "over": dst = src + dst * (1 - src.a).
// Zero coverage outputs 0, which leaves the canvas unchanged exactly. This is the pipeline of
// normal-blend, bilinear layers; composite_ext.wgsl adds bicubic sampling and the other blend
// modes as a second pipeline (WP-2C.04). Display px below are item px (the flip applied).

struct CompositeParams {
  rect: vec4<f32>,     // quad in output px: x0, y0, x1, y1
  view: vec4<f32>,     // output w, output h, opacity, lod
  inv0: vec4<f32>,     // output px -> display px, x row (a, b, c), edge scale x
  inv1: vec4<f32>,     // y row (d, e, f), edge scale y
  tex0: vec4<f32>,     // display px -> level-0 texel, x row
  tex1: vec4<f32>,     // y row
  visible: vec4<f32>,  // visible rectangle in display px: x0, y0, x1, y1
  flags: vec4<u32>,    // mip levels, full cover, blend mode, filter (0 bilinear, 1 bicubic)
}

@group(0) @binding(0) var<uniform> P: CompositeParams;
@group(0) @binding(1) var content: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
  // Two triangles: (0,0) (1,0) (0,1) | (0,1) (1,0) (1,1).
  let right = i == 1u || i == 4u || i == 5u;
  let bottom = i == 2u || i == 3u || i == 5u;
  let px = vec2<f32>(select(P.rect.x, P.rect.z, right), select(P.rect.y, P.rect.w, bottom));
  return vec4<f32>(px.x / P.view.x * 2.0 - 1.0, 1.0 - px.y / P.view.y * 2.0, 0.0, 1.0);
}

fn texel(level: i32, p: vec2<i32>, last: vec2<i32>) -> vec4<f32> {
  return textureLoad(content, clamp(p, vec2<i32>(0), last), level);
}

fn bilinear(level: i32, t: vec2<f32>) -> vec4<f32> {
  let last = vec2<i32>(textureDimensions(content, level)) - vec2<i32>(1);
  let p = t - vec2<f32>(0.5);
  let f0 = floor(p);
  let fr = p - f0;
  let i0 = vec2<i32>(f0);
  let a = texel(level, i0, last);
  let b = texel(level, i0 + vec2<i32>(1, 0), last);
  let c = texel(level, i0 + vec2<i32>(0, 1), last);
  let d = texel(level, i0 + vec2<i32>(1, 1), last);
  let top = a + (b - a) * fr.x;
  let bottom = c + (d - c) * fr.x;
  return top + (bottom - top) * fr.y;
}

fn at_level(level: i32, t: vec2<f32>) -> vec4<f32> {
  let base = vec2<f32>(textureDimensions(content, 0));
  let dims = vec2<f32>(textureDimensions(content, level));
  return bilinear(level, t * (dims / base));
}

fn sample_content(t: vec2<f32>) -> vec4<f32> {
  let lod = P.view.w;
  if (lod <= 0.0) {
    return bilinear(0, t);
  }
  let top = f32(P.flags.x - 1u);
  let fl = floor(lod);
  let l0 = min(fl, top);
  var frac = lod - fl;
  if (fl >= top) {
    frac = 0.0;
  }
  let s0 = at_level(i32(l0), t);
  if (frac == 0.0) {
    return s0;
  }
  let s1 = at_level(i32(l0) + 1, t);
  return s0 + (s1 - s0) * frac;
}

@fragment
fn fs_main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let c = vec3<f32>(frag.xy, 1.0);
  let u = vec2<f32>(dot(P.inv0.xyz, c), dot(P.inv1.xyz, c));
  var cov = 1.0;
  if (P.flags.y == 0u) {
    let dx = min(u.x - P.visible.x, P.visible.z - u.x) / P.inv0.w;
    let dy = min(u.y - P.visible.y, P.visible.w - u.y) / P.inv1.w;
    cov = clamp(dx + 0.5, 0.0, 1.0) * clamp(dy + 0.5, 0.0, 1.0);
    if (cov <= 0.0) {
      return vec4<f32>(0.0);
    }
  }
  let u1 = vec3<f32>(u, 1.0);
  let t = vec2<f32>(dot(P.tex0.xyz, u1), dot(P.tex1.xyz, u1));
  return sample_content(t) * (cov * P.view.z);
}
