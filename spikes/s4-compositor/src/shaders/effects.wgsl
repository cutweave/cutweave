// Small per-pixel passes: fill, shape, dissolve, blur-lite (one direction), 3D LUT.
// Each entry point has its own explicit bind group layout (see gpu.rs).

struct Params {
  size: vec4<u32>,          // w, h, a, b (meaning per entry point)
  v0: vec4<f32>,
  v1: vec4<f32>,
  v2: vec4<f32>,
  weights: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var src_a: texture_2d<f32>;
@group(0) @binding(2) var src_b: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var lut: texture_3d<f32>;

fn in_bounds(gid: vec3<u32>) -> bool {
  return gid.x < P.size.x && gid.y < P.size.y;
}

// v0 = colour.
@compute @workgroup_size(8, 8)
fn fill(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (!in_bounds(gid)) {
    return;
  }
  textureStore(dst, vec2<i32>(gid.xy), P.v0);
}

// v0 = premultiplied colour, v1.x = corner radius.
@compute @workgroup_size(8, 8)
fn shape(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (!in_bounds(gid)) {
    return;
  }
  let half = vec2<f32>(f32(P.size.x), f32(P.size.y)) * 0.5;
  let r = min(P.v1.x, min(half.x, half.y));
  let p = vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5) - half;
  let q = abs(p) - (half - vec2<f32>(r));
  let d = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
  let cov = clamp(0.5 - d, 0.0, 1.0);
  textureStore(dst, vec2<i32>(gid.xy), P.v0 * cov);
}

// v0.x = progress. Linear-light cross-dissolve of premultiplied inputs.
@compute @workgroup_size(8, 8)
fn dissolve(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (!in_bounds(gid)) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let t = P.v0.x;
  textureStore(dst, p, textureLoad(src_a, p, 0) * (1.0 - t) + textureLoad(src_b, p, 0) * t);
}

fn weight(i: u32) -> f32 {
  return P.weights[i / 4u][i % 4u];
}

fn load_clamped(p: vec2<i32>) -> vec4<f32> {
  let dims = vec2<i32>(i32(P.size.x), i32(P.size.y));
  return textureLoad(src_a, clamp(p, vec2<i32>(0), dims - 1), 0);
}

// size.z = 1 horizontal / 0 vertical, size.w = taps each side; weights[0..=taps].
@compute @workgroup_size(8, 8)
fn blur(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (!in_bounds(gid)) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  var d = vec2<i32>(0, 1);
  if (P.size.z == 1u) {
    d = vec2<i32>(1, 0);
  }
  var acc = load_clamped(p) * weight(0u);
  for (var i = 1u; i <= P.size.w; i = i + 1u) {
    let o = d * i32(i);
    acc = acc + weight(i) * (load_clamped(p + o) + load_clamped(p - o));
  }
  textureStore(dst, p, acc);
}

fn lut_load(i: vec3<i32>) -> vec3<f32> {
  let n = i32(P.size.z) - 1;
  return textureLoad(lut, clamp(i, vec3<i32>(0), vec3<i32>(n)), 0).rgb;
}

// size.z = LUT size, size.w = LUT-space transfer; v0.xyz = domain min, v0.w = mix;
// v1.xyz = domain max. Trilinear with exact weights (r, then g, then b).
@compute @workgroup_size(8, 8)
fn lut3d(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (!in_bounds(gid)) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let c = textureLoad(src_a, p, 0);
  if (c.a <= 0.0) {
    textureStore(dst, p, c);
    return;
  }
  let rgb = clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
  let enc = from_linear3(rgb, P.size.w);
  let n1 = f32(P.size.z - 1u);
  let idx = clamp((enc - P.v0.xyz) / (P.v1.xyz - P.v0.xyz), vec3<f32>(0.0), vec3<f32>(1.0)) * n1;
  let f0 = floor(idx);
  let f = idx - f0;
  let i = vec3<i32>(f0);
  let c000 = lut_load(i);
  let c100 = lut_load(i + vec3<i32>(1, 0, 0));
  let c010 = lut_load(i + vec3<i32>(0, 1, 0));
  let c110 = lut_load(i + vec3<i32>(1, 1, 0));
  let c001 = lut_load(i + vec3<i32>(0, 0, 1));
  let c101 = lut_load(i + vec3<i32>(1, 0, 1));
  let c011 = lut_load(i + vec3<i32>(0, 1, 1));
  let c111 = lut_load(i + vec3<i32>(1, 1, 1));
  let c00 = c000 * (1.0 - f.x) + c100 * f.x;
  let c10 = c010 * (1.0 - f.x) + c110 * f.x;
  let c01 = c001 * (1.0 - f.x) + c101 * f.x;
  let c11 = c011 * (1.0 - f.x) + c111 * f.x;
  let c0 = c00 * (1.0 - f.y) + c10 * f.y;
  let c1 = c01 * (1.0 - f.y) + c11 * f.y;
  let graded = c0 * (1.0 - f.z) + c1 * f.z;
  let lin = to_linear3(graded, P.size.w);
  let m = P.v0.w;
  textureStore(dst, p, vec4<f32>((rgb * (1.0 - m) + lin * m) * c.a, c.a));
}
