// Places a layer over the backdrop: inverse affine -> manual bilinear (exact weights, no
// hardware sampler) -> 1/2-px edge coverage -> opacity -> W3C blend on premultiplied colour.

struct CompositeParams {
  inv0: vec4<f32>,  // canvas -> layer: x' = inv0.x*x + inv0.y*y + inv0.z
  inv1: vec4<f32>,  //                  y' = inv1.x*x + inv1.y*y + inv1.z
  layer: vec4<f32>, // layer w, layer h, opacity, -
  mode: vec4<u32>,  // blend mode, canvas w, canvas h, -
}

@group(0) @binding(0) var<uniform> P: CompositeParams;
@group(0) @binding(1) var backdrop: texture_2d<f32>;
@group(0) @binding(2) var layer_tex: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn load_layer(x: i32, y: i32) -> vec4<f32> {
  let dims = vec2<i32>(textureDimensions(layer_tex));
  return textureLoad(layer_tex, clamp(vec2<i32>(x, y), vec2<i32>(0), dims - 1), 0);
}

fn sample_layer(u: vec2<f32>) -> vec4<f32> {
  let p = u - vec2<f32>(0.5);
  let f0 = floor(p);
  let fr = p - f0;
  let i0 = vec2<i32>(f0);
  let a = load_layer(i0.x, i0.y);
  let b = load_layer(i0.x + 1, i0.y);
  let c = load_layer(i0.x, i0.y + 1);
  let d = load_layer(i0.x + 1, i0.y + 1);
  let top = a * (1.0 - fr.x) + b * fr.x;
  let bottom = c * (1.0 - fr.x) + d * fr.x;
  return top * (1.0 - fr.y) + bottom * fr.y;
}

fn screen(b: f32, s: f32) -> f32 {
  return b + s - b * s;
}

fn blend_channel(cb: f32, cs: f32, mode: u32) -> f32 {
  let b = clamp(cb, 0.0, 1.0);
  let s = clamp(cs, 0.0, 1.0);
  switch mode {
    case 1u: {
      return cb * cs;
    }
    case 2u: {
      return screen(b, s);
    }
    case 3u: {
      if (b <= 0.5) {
        return s * 2.0 * b;
      }
      return screen(s, 2.0 * b - 1.0);
    }
    case 4u: {
      return min(cb, cs);
    }
    case 5u: {
      return max(cb, cs);
    }
    case 6u: {
      return cb + cs;
    }
    case 7u: {
      if (s <= 0.5) {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
      }
      var d = sqrt(b);
      if (b <= 0.25) {
        d = ((16.0 * b - 12.0) * b + 4.0) * b;
      }
      return b + (2.0 * s - 1.0) * (d - b);
    }
    default: {
      return cs;
    }
  }
}

fn blend(dstc: vec4<f32>, src: vec4<f32>, mode: u32) -> vec4<f32> {
  let sa = src.a;
  let da = dstc.a;
  if (mode == 0u) {
    return src + dstc * (1.0 - sa);
  }
  var cs = vec3<f32>(0.0);
  if (sa > 0.0) {
    cs = src.rgb / max(sa, 1e-8);
  }
  var cb = vec3<f32>(0.0);
  if (da > 0.0) {
    cb = dstc.rgb / max(da, 1e-8);
  }
  let bl = vec3<f32>(
    blend_channel(cb.r, cs.r, mode),
    blend_channel(cb.g, cs.g, mode),
    blend_channel(cb.b, cs.b, mode),
  );
  let rgb = src.rgb * (1.0 - da) + dstc.rgb * (1.0 - sa) + sa * da * bl;
  return vec4<f32>(rgb, sa + da * (1.0 - sa));
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
  if (gid.x >= P.mode.y || gid.y >= P.mode.z) {
    return;
  }
  let c = vec3<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5, 1.0);
  let u = vec2<f32>(dot(P.inv0.xyz, c), dot(P.inv1.xyz, c));
  let size = P.layer.xy;
  let cov = clamp(min(u.x, size.x - u.x) + 0.5, 0.0, 1.0)
    * clamp(min(u.y, size.y - u.y) + 0.5, 0.0, 1.0);
  var src = vec4<f32>(0.0);
  if (cov > 0.0) {
    src = sample_layer(u) * (cov * P.layer.z);
  }
  let p = vec2<i32>(gid.xy);
  textureStore(dst, p, blend(textureLoad(backdrop, p, 0), src, P.mode.x));
}
