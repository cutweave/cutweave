// SPDX-License-Identifier: GPL-3.0-or-later
// Input transform: decoded planes -> premultiplied linear Rec.709 (Rgba16Float, mip 0).
// codes -> range -> matrix -> clamp [0,1] -> transfer^-1 -> (HLG OOTF) -> primaries
// -> (PQ/HLG: BT.2390 EETF on max RGB) -> clip negatives -> premultiply -> round to the nearest half
// float (common.wgsl round_half: the store is then exact on every adapter).
// Planes are integer textures read with textureLoad; 4:2:0 chroma is upsampled with explicit
// bilinear weights (a + (b - a) * f) at the frame's chroma siting, clamp-to-edge.

struct DecodeParams {
  size: vec4<u32>,     // luma w, luma h, chroma w, chroma h
  mode: vec4<u32>,     // layout (0 semi-planar, 1 planar, 2 rgba), code shift, transfer, flags
  range: vec4<f32>,    // y_off, y_scale, c_off, c_scale
  coeffs: vec4<f32>,   // kr, kb, kg, linear scale
  siting: vec4<f32>,   // chroma offset x, y (luma samples), HLG gamma - 1, -
  prim0: vec4<f32>,    // source primaries -> BT.709, rows
  prim1: vec4<f32>,
  prim2: vec4<f32>,
  tm0: vec4<f32>,      // src_min_pq, src_max_pq, min_lum, max_lum
  tm1: vec4<f32>,      // ks, sdr white nits, -, -
  hlg: vec4<f32>,      // OOTF luma coefficients of the source primaries
}

const FLAG_TONE_MAP: u32 = 1u;
const FLAG_HLG_OOTF: u32 = 2u;

@group(0) @binding(0) var<uniform> P: DecodeParams;
@group(0) @binding(1) var plane0: texture_2d<u32>;
@group(0) @binding(2) var plane1: texture_2d<u32>;
@group(0) @binding(3) var plane2: texture_2d<u32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

fn chroma_codes(p: vec2<i32>) -> vec2<f32> {
  let c = clamp(p, vec2<i32>(0), vec2<i32>(P.size.zw) - vec2<i32>(1));
  if (P.mode.x == 0u) {
    return vec2<f32>(textureLoad(plane1, c, 0).rg >> vec2<u32>(P.mode.y));
  }
  return vec2<f32>(f32(textureLoad(plane1, c, 0).r), f32(textureLoad(plane2, c, 0).r));
}

fn chroma(pos: vec2<f32>) -> vec2<f32> {
  let f0 = floor(pos);
  let fr = pos - f0;
  let i0 = vec2<i32>(f0);
  let a = chroma_codes(i0);
  let b = chroma_codes(i0 + vec2<i32>(1, 0));
  let c = chroma_codes(i0 + vec2<i32>(0, 1));
  let d = chroma_codes(i0 + vec2<i32>(1, 1));
  let top = a + (b - a) * fr.x;
  let bottom = c + (d - c) * fr.x;
  return top + (bottom - top) * fr.y;
}

fn eetf(e1: f32) -> f32 {
  let src_min = P.tm0.x;
  let src_max = P.tm0.y;
  let e1n = clamp((e1 - src_min) / (src_max - src_min), 0.0, 1.0);
  let ks = P.tm1.x;
  var e2 = e1n;
  if (e1n > ks && ks < 1.0) {
    let t = (e1n - ks) / (1.0 - ks);
    let t2 = t * t;
    let t3 = t2 * t;
    e2 = (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks)
      + (-2.0 * t3 + 3.0 * t2) * P.tm0.w;
  }
  let k = 1.0 - e2;
  let e3 = e2 + P.tm0.z * (k * k * k * k);
  return e3 * (src_max - src_min) + src_min;
}

fn tone_map(rgb_nits: vec3<f32>) -> vec3<f32> {
  let m = max(max(rgb_nits.r, rgb_nits.g), rgb_nits.b);
  if (m <= 0.0) {
    return vec3<f32>(0.0);
  }
  let m_out = pq_eotf(eetf(pq_inv_eotf(m / 10000.0))) * 10000.0;
  return rgb_nits * (m_out / m / P.tm1.y);
}

fn finish(signal: vec3<f32>) -> vec3<f32> {
  let s = clamp(signal, vec3<f32>(0.0), vec3<f32>(1.0));
  let tf = P.mode.z;
  let lin = vec3<f32>(to_linear(s.r, tf), to_linear(s.g, tf), to_linear(s.b, tf));
  var scale = P.coeffs.w;
  if ((P.mode.w & FLAG_HLG_OOTF) != 0u) {
    scale = scale * safe_pow(dot(P.hlg.xyz, lin), P.siting.z);
  }
  let l = lin * scale;
  var out = vec3<f32>(dot(P.prim0.xyz, l), dot(P.prim1.xyz, l), dot(P.prim2.xyz, l));
  if ((P.mode.w & FLAG_TONE_MAP) != 0u) {
    out = tone_map(out);
  }
  return max(out, vec3<f32>(0.0));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  if (P.mode.x == 2u) {
    let c = textureLoad(plane0, p, 0);
    let sig = (vec3<f32>(c.rgb) - vec3<f32>(P.range.x)) / P.range.y;
    // Exactly 1 for opaque pixels, so opaque layers composite exactly.
    let a = select(f32(c.a) * (1.0 / 255.0), 1.0, c.a == 255u);
    textureStore(dst, p, round_half4(vec4<f32>(finish(sig) * a, a)));
    return;
  }
  let yc = f32(textureLoad(plane0, p, 0).r >> P.mode.y);
  let cc = chroma((vec2<f32>(gid.xy) - P.siting.xy) * 0.5);
  let yn = (yc - P.range.x) / P.range.y;
  let cb = (cc.x - P.range.z) / P.range.w;
  let cr = (cc.y - P.range.z) / P.range.w;
  let kr = P.coeffs.x;
  let kb = P.coeffs.y;
  let r = yn + 2.0 * (1.0 - kr) * cr;
  let b = yn + 2.0 * (1.0 - kb) * cb;
  let g = (yn - kr * r - kb * b) / P.coeffs.z;
  textureStore(dst, p, round_half4(vec4<f32>(finish(vec3<f32>(r, g, b)), 1.0)));
}
