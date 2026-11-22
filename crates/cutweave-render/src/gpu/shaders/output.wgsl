// SPDX-License-Identifier: GPL-3.0-or-later
// Output transform: premultiplied linear -> straight alpha -> clamp -> encode -> explicit
// round-half-up quantisation into a packed RGBA8 storage buffer (never the hardware float ->
// unorm conversion of a render target: S4 finding 1).

struct OutParams {
  size: vec4<u32>, // w, h, transfer id, -
}

@group(0) @binding(0) var<uniform> P: OutParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> out_buf: array<u32>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let c = textureLoad(src, vec2<i32>(gid.xy), 0);
  let a = clamp(c.a, 0.0, 1.0);
  var rgb = vec3<f32>(0.0);
  if (a >= 1.0) {
    rgb = c.rgb;
  } else if (a > 0.0) {
    rgb = c.rgb / a;
  }
  let l = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
  let tf = P.size.z;
  let enc = vec4<f32>(from_linear(l.r, tf), from_linear(l.g, tf), from_linear(l.b, tf), a);
  let q = vec4<u32>(clamp(floor(enc * 255.0 + 0.5), vec4<f32>(0.0), vec4<f32>(255.0)));
  out_buf[gid.y * P.size.x + gid.x] = q.r | (q.g << 8u) | (q.b << 16u) | (q.a << 24u);
}
