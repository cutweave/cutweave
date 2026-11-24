// SPDX-License-Identifier: GPL-3.0-or-later
// Path (a): planes uploaded from system memory as integer textures (R8/RG8 or R16/RG16 Uint).
// NV12/P010 put interleaved CbCr in `plane_c0`; planar formats put Cb in `plane_c0` and Cr in
// `plane_c1`. P010 samples are MSB-aligned and shifted down by `P.mode.y`.

@group(0) @binding(1) var plane_y: texture_2d<u32>;
@group(0) @binding(2) var plane_c0: texture_2d<u32>;
@group(0) @binding(3) var plane_c1: texture_2d<u32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba8unorm, write>;

fn load_c(x: i32, y: i32) -> vec2<f32> {
  let p = clamp_c(x, y);
  let s = P.mode.y;
  if (P.mode.x == 0u) {
    let v = textureLoad(plane_c0, p, 0).rg >> vec2<u32>(s, s);
    return vec2<f32>(v);
  }
  let cb = textureLoad(plane_c0, p, 0).r >> s;
  let cr = textureLoad(plane_c1, p, 0).r >> s;
  return vec2<f32>(f32(cb), f32(cr));
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
fn convert(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.size.x || gid.y >= P.size.y) {
    return;
  }
  let p = vec2<i32>(gid.xy);
  let yc = f32(textureLoad(plane_y, p, 0).r >> P.mode.y);
  textureStore(dst, p, to_rgb(yc, chroma(chroma_pos(gid.xy))));
}
