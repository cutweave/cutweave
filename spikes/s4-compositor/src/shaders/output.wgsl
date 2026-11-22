// Output transform: linear -> clamp -> encode -> explicit round-half-up quantisation into a
// packed storage buffer (no reliance on the hardware float->unorm conversion).

struct OutParams {
  size: vec4<u32>, // w, h, transfer id, bit depth
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
  let enc = from_linear3(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), P.size.z);
  let v = vec4<f32>(enc, clamp(c.a, 0.0, 1.0));
  let idx = gid.y * P.size.x + gid.x;
  if (P.size.w == 16u) {
    let q = vec4<u32>(clamp(floor(v * 65535.0 + 0.5), vec4<f32>(0.0), vec4<f32>(65535.0)));
    out_buf[idx * 2u] = q.r | (q.g << 16u);
    out_buf[idx * 2u + 1u] = q.b | (q.a << 16u);
  } else {
    let q = vec4<u32>(clamp(floor(v * 255.0 + 0.5), vec4<f32>(0.0), vec4<f32>(255.0)));
    out_buf[idx] = q.r | (q.g << 8u) | (q.b << 16u) | (q.a << 24u);
  }
}
