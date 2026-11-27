// SPDX-License-Identifier: GPL-3.0-or-later
// One direction of the background's separable Gaussian blur (WP-2B.02, src/backdrop.rs): for
// each texel, the texels i = -taps..taps along the axis (edges clamped) weighted exp(-i^2 /
// 2 sigma^2) and normalised; then the colour (not alpha) times 1 - dim. taps = 0 copies.
// Premultiplied linear light in and out, rounded to the nearest half float (common.wgsl).

struct BlurParams {
  size: vec4<u32>, // w, h, taps, 1 = along x / 0 = along y
  k: vec4<f32>,    // 1 / (2 sigma^2), 1 - dim, -, -
}

@group(0) @binding(0) var<uniform> P: BlurParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let last = vec2<i32>(P.size.xy) - vec2<i32>(1);
  let p = vec2<i32>(gid.xy);
  let taps = i32(P.size.z);
  var step = vec2<i32>(0, 1);
  if (P.size.w == 1u) {
    step = vec2<i32>(1, 0);
  }
  var acc = vec4<f32>(0.0);
  var total = 0.0;
  for (var i = -taps; i <= taps; i = i + 1) {
    let w = exp(-f32(i * i) * P.k.x);
    acc = acc + textureLoad(src, clamp(p + step * i, vec2<i32>(0), last), 0) * w;
    total = total + w;
  }
  let c = acc / total;
  textureStore(dst, p, round_half4(vec4<f32>(c.rgb * P.k.y, c.a)));
}
