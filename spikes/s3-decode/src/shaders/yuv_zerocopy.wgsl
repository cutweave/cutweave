// SPDX-License-Identifier: GPL-3.0-or-later
// Path (b): the decoder's NV12/P010 texture imported into wgpu, read through its plane views
// (Plane0 = R8/R16Unorm luma, Plane1 = RG8/RG16Unorm CbCr). Unorm values are turned back into
// code values: round(v * max_code) >> shift (P010: 65535, 6).

@group(0) @binding(1) var plane_y: texture_2d<f32>;
@group(0) @binding(2) var plane_c: texture_2d<f32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba8unorm, write>;

fn code(v: f32) -> f32 {
  return floor(v * f32(P.mode.z) + 0.5) / f32(1u << P.mode.y);
}

fn load_c(x: i32, y: i32) -> vec2<f32> {
  let v = textureLoad(plane_c, clamp_c(x, y), 0).rg;
  return vec2<f32>(code(v.x), code(v.y));
}

fn chroma(pos: vec2<f32>) -> vec2<f32> {
  let f0 = floor(pos);
  let fr = pos - f0;
  let i0 = vec2<i32>(f0);
  let top = mix(load_c(i0.x, i0.y), load_c(i0.x + 1, i0.y), fr.x);
  let bottom = mix(load_c(i0.x, i0.y + 1), load_c(i0.x + 1, i0.y + 1), fr.x);
  return mix(top, bottom, fr.y);
}

@compute @workgroup_size(8, 8)
fn convert_planes(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let yc = code(textureLoad(plane_y, p, 0).r);
  textureStore(dst, p, to_rgb(yc, chroma(chroma_pos(gid.xy))));
}
