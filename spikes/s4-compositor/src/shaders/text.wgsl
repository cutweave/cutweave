// MTSDF text pass (WP-0F.02): one instanced draw of glyph quads, ordered shadow layer →
// outline layer → fill layer, each quad sampling the glyph's own atlas cell with exact-weight
// bilinear taps (textureLoad, clamped to the cell: no bleeding, placement-independent). Output is
// premultiplied linear colour, blended with fixed-function "over" (One, OneMinusSrcAlpha) into
// an Rgba16Float target: primitive order is API order, so a given adapter is deterministic.
//
// Field texels: RGB = MSDF (sharp corners, used for the fill), A = true SDF (round outline and
// shadow). Value 0.5 = edge, +1 = range_px atlas pixels inside.

struct TextParams {
  t0: vec4<f32>,     // box -> target affine, row 0 (a, b, c, -)
  t1: vec4<f32>,     // row 1 (d, e, f, -)
  dims: vec4<f32>,   // target w, h, box w, box h
  field: vec4<f32>,  // range_px, em_px, -, -
}

struct Glyph {
  anim0: vec4<f32>,  // layout -> box affine row 0 (a, b, c), kind (0 fill, 1 outline, 2 shadow)
  anim1: vec4<f32>,  // row 1 (d, e, f), em scale = size / em_px (layout px per atlas px)
  quad: vec4<f32>,   // layout-space quad x0, y0, x1, y1 (= the cell's plane rect)
  cell: vec4<f32>,   // atlas texel rect x0, y0, x1, y1
  color: vec4<f32>,  // premultiplied linear colour x opacity
  hl: vec4<f32>,     // karaoke colour x opacity (premultiplied)
  params: vec4<f32>, // outline width (layout px), shadow softness (target px), sweep x, sweep dir
}

@group(0) @binding(0) var<uniform> P: TextParams;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> glyphs: array<Glyph>;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) layout_p: vec2<f32>,
  @location(2) box_p: vec2<f32>,
  @location(3) @interpolate(flat) idx: u32,
  @location(4) @interpolate(flat) scale: f32,
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
  let g = glyphs[ii];
  var corners = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
  );
  let c = corners[vi];
  let lp = mix(g.quad.xy, g.quad.zw, c);
  let bp = vec2<f32>(dot(g.anim0.xyz, vec3<f32>(lp, 1.0)), dot(g.anim1.xyz, vec3<f32>(lp, 1.0)));
  let tp = vec2<f32>(dot(P.t0.xyz, vec3<f32>(bp, 1.0)), dot(P.t1.xyz, vec3<f32>(bp, 1.0)));
  var out: VsOut;
  out.pos = vec4<f32>(tp.x / P.dims.x * 2.0 - 1.0, 1.0 - tp.y / P.dims.y * 2.0, 0.0, 1.0);
  out.uv = mix(g.cell.xy, g.cell.zw, c);
  out.layout_p = lp;
  out.box_p = bp;
  out.idx = ii;
  let det_a = g.anim0.x * g.anim1.y - g.anim0.y * g.anim1.x;
  let det_t = P.t0.x * P.t1.y - P.t0.y * P.t1.x;
  out.scale = sqrt(abs(det_a * det_t)); // target px per layout px
  return out;
}

fn load_cell(p: vec2<i32>, lo: vec2<i32>, hi: vec2<i32>) -> vec4<f32> {
  return textureLoad(atlas, clamp(p, lo, hi), 0);
}

fn sample_field(uv: vec2<f32>, cell: vec4<f32>) -> vec4<f32> {
  let lo = vec2<i32>(cell.xy);
  let hi = vec2<i32>(cell.zw) - vec2<i32>(1);
  let p = uv - vec2<f32>(0.5);
  let f0 = floor(p);
  let fr = p - f0;
  let i0 = vec2<i32>(f0);
  let a = load_cell(i0, lo, hi);
  let b = load_cell(i0 + vec2<i32>(1, 0), lo, hi);
  let c = load_cell(i0 + vec2<i32>(0, 1), lo, hi);
  let d = load_cell(i0 + vec2<i32>(1, 1), lo, hi);
  let top = a * (1.0 - fr.x) + b * fr.x;
  let bottom = c * (1.0 - fr.x) + d * fr.x;
  return top * (1.0 - fr.y) + bottom * fr.y;
}

fn median3(a: f32, b: f32, c: f32) -> f32 {
  return max(min(a, b), min(max(a, b), c));
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
  // Derivatives first (uniform control flow).
  let dx = max(fwidth(in.layout_p.x), 1e-4);
  let g = glyphs[in.idx];
  let kind = u32(g.anim0.w);
  let s = sample_field(in.uv, g.cell);
  // Target px per normalised field unit (range_px atlas px).
  let ppu = P.field.x * g.anim1.w * in.scale;
  let sd_true = (s.a - 0.5) * ppu;
  var col = g.color;
  var cov = 0.0;
  if (kind == 0u) {
    // MSDF fill; the true SDF bounds far-field MSDF artefacts.
    let sd = min((median3(s.r, s.g, s.b) - 0.5) * max(ppu, 1.0), sd_true + ppu / P.field.x);
    cov = clamp(sd + 0.5, 0.0, 1.0);
    if (g.params.w != 0.0) {
      let t = clamp((g.params.z - in.layout_p.x) * g.params.w / dx + 0.5, 0.0, 1.0);
      col = mix(g.color, g.hl, t);
    }
  } else {
    let widen = g.params.x * in.scale;
    var soft = 1.0;
    if (kind == 2u) {
      soft = max(g.params.y, 1.0);
    }
    cov = clamp((sd_true + widen) / soft + 0.5, 0.0, 1.0);
  }
  let bc = clamp(min(in.box_p, P.dims.zw - in.box_p) + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
  return col * (cov * bc.x * bc.y);
}
