// SPDX-License-Identifier: GPL-3.0-or-later
// Y'CbCr 4:2:0 -> display-referred R'G'B' (WP-0E.02). The maths is the input stage of Spike S4's
// decode.wgsl (range -> matrix, MPEG-2 chroma siting, bilinear chroma), without the transfer
// function, primaries and tone-map stages: S3 only proves decode -> texture, the colour pipeline
// is S4's. Code values arrive already LSB-aligned.

struct Params {
  size: vec4<u32>,   // luma w, luma h, chroma w, chroma h
  mode: vec4<u32>,   // layout (0 semi-planar CbCr, 1 planar Cb + Cr), right shift, max code, -
  range: vec4<f32>,  // y_off, y_scale, c_off, c_scale (code values)
  coeffs: vec4<f32>, // kr, kb, kg, -
}

@group(0) @binding(0) var<uniform> P: Params;

// MPEG-2 siting: chroma sample (i, j) sits at luma (2i, 2j + 0.5).
fn chroma_pos(gid: vec2<u32>) -> vec2<f32> {
  return vec2<f32>(f32(gid.x) * 0.5, (f32(gid.y) - 0.5) * 0.5);
}

fn clamp_c(x: i32, y: i32) -> vec2<i32> {
  return vec2<i32>(clamp(x, 0, i32(P.size.z) - 1), clamp(y, 0, i32(P.size.w) - 1));
}

fn to_rgb(yc: f32, c: vec2<f32>) -> vec4<f32> {
  let yn = (yc - P.range.x) / P.range.y;
  let cb = (c.x - P.range.z) / P.range.w;
  let cr = (c.y - P.range.z) / P.range.w;
  let kr = P.coeffs.x;
  let kb = P.coeffs.y;
  let r = yn + 2.0 * (1.0 - kr) * cr;
  let b = yn + 2.0 * (1.0 - kb) * cb;
  let g = (yn - kr * r - kb * b) / P.coeffs.z;
  return vec4<f32>(clamp(vec3<f32>(r, g, b), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
