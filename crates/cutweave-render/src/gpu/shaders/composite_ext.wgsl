// SPDX-License-Identifier: GPL-3.0-or-later
// The composite pass for layers that need more than bilinear "over" (WP-2C.04): bicubic
// resampling (the export option) and the non-normal blend modes. Appended to composite.wgsl
// (same uniforms, vertex stage and bilinear helpers) as a pipeline of its own, so the plain
// pipeline's shader stays exactly as it was. Written independently of the f64 CPU reference
// (src/reference.rs, src/scene.rs `BlendMode::blend`).
//
// The fragment composites in the shader and replaces the canvas pixel (no fixed-function
// blending): it reads the canvas below the layer from `backdrop`, a copy of the layer's output
// rectangle made just before the draw. With s the layer's premultiplied sample scaled by
// coverage x opacity and d the backdrop, the W3C separable formula in linear light is
//   rgb = s.rgb * (1 - d.a) + d.rgb * (1 - s.a) + s.a * d.a * B(d.rgb / d.a, s.rgb / s.a)
//   a   = s.a + d.a * (1 - s.a)
// (normal, B = Cs: "over"), rounded to the nearest half float so the store is exact on every
// adapter. A pixel the layer does not cover gets the backdrop back, bit for bit.

@group(0) @binding(2) var backdrop: texture_2d<f32>;

// Catmull-Rom (cubic convolution, a = -1/2) weights of the 4 texels around a sample at
// fraction f past the second one. Exactly (0, 1, 0, 0) at f = 0.
fn cubic_weights(f: f32) -> vec4<f32> {
  let f2 = f * f;
  let f3 = f2 * f;
  return vec4<f32>(
    f2 - 0.5 * f3 - 0.5 * f,
    1.5 * f3 - 2.5 * f2 + 1.0,
    2.0 * f2 - 1.5 * f3 + 0.5 * f,
    0.5 * f3 - 0.5 * f2,
  );
}

fn bicubic(level: i32, t: vec2<f32>) -> vec4<f32> {
  let last = vec2<i32>(textureDimensions(content, level)) - vec2<i32>(1);
  let p = t - vec2<f32>(0.5);
  let f0 = floor(p);
  let fr = p - f0;
  let i0 = vec2<i32>(f0);
  let wx = cubic_weights(fr.x);
  let wy = cubic_weights(fr.y);
  var acc = vec4<f32>(0.0);
  for (var j = 0; j < 4; j++) {
    let y = i0.y - 1 + j;
    let row = texel(level, vec2<i32>(i0.x - 1, y), last) * wx.x
      + texel(level, vec2<i32>(i0.x, y), last) * wx.y
      + texel(level, vec2<i32>(i0.x + 1, y), last) * wx.z
      + texel(level, vec2<i32>(i0.x + 2, y), last) * wx.w;
    acc += row * wy[j];
  }
  // The negative lobes ring at sharp edges: no negative light, alpha within [0, 1].
  return vec4<f32>(max(acc.rgb, vec3<f32>(0.0)), clamp(acc.a, 0.0, 1.0));
}

fn at_level_bicubic(level: i32, t: vec2<f32>) -> vec4<f32> {
  let base = vec2<f32>(textureDimensions(content, 0));
  let dims = vec2<f32>(textureDimensions(content, level));
  return bicubic(level, t * (dims / base));
}

// sample_content's level selection (trilinear between mips when minifying), bicubic per level.
fn sample_bicubic(t: vec2<f32>) -> vec4<f32> {
  let lod = P.view.w;
  if (lod <= 0.0) {
    return bicubic(0, t);
  }
  let top = f32(P.flags.x - 1u);
  let fl = floor(lod);
  let l0 = min(fl, top);
  var frac = lod - fl;
  if (fl >= top) {
    frac = 0.0;
  }
  let s0 = at_level_bicubic(i32(l0), t);
  if (frac == 0.0) {
    return s0;
  }
  let s1 = at_level_bicubic(i32(l0) + 1, t);
  return s0 + (s1 - s0) * frac;
}

// B(Cb, Cs) of blend mode `mode` (BlendMode::shader_id) on one straight channel, both inputs
// clamped to [0, 1] (the modes' SDR domain).
fn blend_channel(mode: u32, cb: f32, cs: f32) -> f32 {
  let b = clamp(cb, 0.0, 1.0);
  let s = clamp(cs, 0.0, 1.0);
  switch mode {
    case 1u: {
      return b * s;
    }
    case 2u: {
      return b + s - b * s;
    }
    case 3u: {
      if (b <= 0.5) {
        return 2.0 * b * s;
      }
      let h = 2.0 * b - 1.0;
      return s + h - s * h;
    }
    case 4u: {
      return min(b, s);
    }
    case 5u: {
      return max(b, s);
    }
    case 6u: {
      return min(b + s, 1.0);
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

@fragment
fn fs_ext(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
  let d = textureLoad(backdrop, vec2<i32>(frag.xy), 0);
  let c = vec3<f32>(frag.xy, 1.0);
  let u = vec2<f32>(dot(P.inv0.xyz, c), dot(P.inv1.xyz, c));
  var cov = 1.0;
  if (P.flags.y == 0u) {
    let dx = min(u.x - P.visible.x, P.visible.z - u.x) / P.inv0.w;
    let dy = min(u.y - P.visible.y, P.visible.w - u.y) / P.inv1.w;
    cov = clamp(dx + 0.5, 0.0, 1.0) * clamp(dy + 0.5, 0.0, 1.0);
    if (cov <= 0.0) {
      return d;
    }
  }
  let u1 = vec3<f32>(u, 1.0);
  let t = vec2<f32>(dot(P.tex0.xyz, u1), dot(P.tex1.xyz, u1));
  var sample: vec4<f32>;
  if (P.flags.w == 1u) {
    sample = sample_bicubic(t);
  } else {
    sample = sample_content(t);
  }
  let s = sample * (cov * P.view.z);
  let mode = P.flags.z;
  if (mode == 0u || s.a <= 0.0 || d.a <= 0.0) {
    return round_half4(s + d * (1.0 - s.a));
  }
  let cs = s.rgb / s.a;
  let cb = d.rgb / d.a;
  let b = vec3<f32>(
    blend_channel(mode, cb.r, cs.r),
    blend_channel(mode, cb.g, cs.g),
    blend_channel(mode, cb.b, cs.b),
  );
  let rgb = s.rgb * (1.0 - d.a) + d.rgb * (1.0 - s.a) + (s.a * d.a) * b;
  return round_half4(vec4<f32>(rgb, s.a + d.a * (1.0 - s.a)));
}
