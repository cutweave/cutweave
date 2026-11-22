// SPDX-License-Identifier: GPL-3.0-or-later
// One mip level: the 2x2 box average of the level above (edges clamped), size max(1, size >> 1).

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  let d = textureDimensions(dst);
  if (gid.x >= d.x || gid.y >= d.y) {
    return;
  }
  let last = vec2<i32>(textureDimensions(src)) - vec2<i32>(1);
  let p = vec2<i32>(gid.xy) * 2;
  let a = textureLoad(src, min(p, last), 0);
  let b = textureLoad(src, min(p + vec2<i32>(1, 0), last), 0);
  let c = textureLoad(src, min(p + vec2<i32>(0, 1), last), 0);
  let e = textureLoad(src, min(p + vec2<i32>(1, 1), last), 0);
  textureStore(dst, vec2<i32>(gid.xy), (a + b + c + e) * 0.25);
}
