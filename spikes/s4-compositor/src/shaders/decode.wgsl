// Input colour transform: decoded planes -> premultiplied linear Rec.709 (Rgba16Float).
// range -> matrix -> clamp -> transfer^-1 -> primaries -> (PQ: BT.2390 EETF on max RGB).

struct DecodeParams {
  size: vec4<u32>,   // luma w, luma h, chroma w, chroma h
  mode: vec4<u32>,   // transfer id, tone-map on, -, -
  range: vec4<f32>,  // y_off, y_scale, c_off, c_scale
  coeffs: vec4<f32>, // kr, kb, kg, linear scale (10000 for PQ)
  prim0: vec4<f32>,  // primaries conversion matrix rows
  prim1: vec4<f32>,
  prim2: vec4<f32>,
  tm0: vec4<f32>,    // src_min_pq, src_max_pq, min_lum, max_lum
  tm1: vec4<f32>,    // ks, sdr_white_nits, -, -
}

@group(0) @binding(0) var<uniform> P: DecodeParams;
@group(0) @binding(1) var plane_y: texture_2d<u32>;
@group(0) @binding(2) var plane_cb: texture_2d<u32>;
@group(0) @binding(3) var plane_cr: texture_2d<u32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var rgba_src: texture_2d<f32>;

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

fn finish(rgb_signal: vec3<f32>) -> vec3<f32> {
  let s = clamp(rgb_signal, vec3<f32>(0.0), vec3<f32>(1.0));
  let lin = to_linear3(s, P.mode.x) * P.coeffs.w;
  var out = vec3<f32>(dot(P.prim0.xyz, lin), dot(P.prim1.xyz, lin), dot(P.prim2.xyz, lin));
  if (P.mode.y == 1u) {
    out = tone_map(out);
  }
  return max(out, vec3<f32>(0.0));
}

fn load_chroma(plane: texture_2d<u32>, x: i32, y: i32) -> f32 {
  let cx = clamp(x, 0, i32(P.size.z) - 1);
  let cy = clamp(y, 0, i32(P.size.w) - 1);
  return f32(textureLoad(plane, vec2<i32>(cx, cy), 0).r);
}

// MPEG-2 siting: chroma sample (i, j) sits at luma (2i, 2j + 0.5).
fn chroma(plane: texture_2d<u32>, pos: vec2<f32>) -> f32 {
  let f0 = floor(pos);
  let fr = pos - f0;
  let i0 = vec2<i32>(f0);
  let a = load_chroma(plane, i0.x, i0.y);
  let b = load_chroma(plane, i0.x + 1, i0.y);
  let c = load_chroma(plane, i0.x, i0.y + 1);
  let d = load_chroma(plane, i0.x + 1, i0.y + 1);
  let top = a * (1.0 - fr.x) + b * fr.x;
  let bottom = c * (1.0 - fr.x) + d * fr.x;
  return top * (1.0 - fr.y) + bottom * fr.y;
}

@compute @workgroup_size(8, 8)
fn decode_yuv420(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let pos = vec2<f32>(f32(gid.x) * 0.5, (f32(gid.y) - 0.5) * 0.5);
  let yc = f32(textureLoad(plane_y, p, 0).r);
  let yn = (yc - P.range.x) / P.range.y;
  let cb = (chroma(plane_cb, pos) - P.range.z) / P.range.w;
  let cr = (chroma(plane_cr, pos) - P.range.z) / P.range.w;
  let kr = P.coeffs.x;
  let kb = P.coeffs.y;
  let r = yn + 2.0 * (1.0 - kr) * cr;
  let b = yn + 2.0 * (1.0 - kb) * cb;
  let g = (yn - kr * r - kb * b) / P.coeffs.z;
  textureStore(dst, p, vec4<f32>(finish(vec3<f32>(r, g, b)), 1.0));
}

@compute @workgroup_size(8, 8)
fn decode_rgba8(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let c = textureLoad(rgba_src, p, 0);
  textureStore(dst, p, vec4<f32>(finish(c.rgb) * c.a, c.a));
}
