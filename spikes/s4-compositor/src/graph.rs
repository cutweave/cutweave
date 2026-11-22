//! Render graph: a DAG of passes compiled from a [`Scene`] (§8: "cull · order · allocate ·
//! cache keys").
//!
//! * **Passes** read 0..n texture resources and write exactly one. Every content node lowers to a
//!   pass producing a premultiplied linear `Rgba16Float` texture of its intrinsic size; effects
//!   and composites are passes over those.
//! * **Cache keys** are Merkle hashes: `key(pass) = H(op, params, size, key(inputs)…)`. A decode
//!   of a static source therefore has the same key on every frame, and the executor reuses its
//!   texture instead of re-uploading and re-converting it. The output key identifies the frame's
//!   whole subgraph, i.e. the `(subgraph hash, t)` frame-cache key of §8.
//! * **Culling** happens at build time: zero-opacity layers, degenerate transforms, layers fully
//!   outside the canvas, and transitions at progress 0/1 (one side only). Unreachable passes are
//!   pruned afterwards.
//! * **Order** is a deterministic Kahn topological sort; the executor derives texture lifetimes
//!   from it and recycles transient textures.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::color::srgb_fill_to_linear_premul;
use crate::hash::{CacheKey, Fnv};
use crate::lut::Lut3d;
use crate::scene::{
    Affine, BlendMode, ColorTags, Effect, Layer, Node, Scene, TextNode, ToneMap, Transfer,
    TransitionNode, apply_affine, compose_affine, invert_affine,
};
use crate::source::{PixelData, SourceFrame, SourceProvider};
use crate::{Error, Result};

/// Index into [`RenderGraph::resources`].
pub type ResId = usize;

/// A 2D `Rgba16Float` texture (or, for the output pass, the readback buffer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub width: u32,
    pub height: u32,
}

/// What a pass does.
#[derive(Clone, Debug, PartialEq)]
pub enum PassOp {
    /// Constant premultiplied linear colour.
    Fill { color: [f32; 4] },
    /// Upload a source frame and apply the input colour transform (range → matrix → transfer →
    /// primaries → optional PQ tone-map) into linear Rec.709, premultiplied.
    Decode {
        source: usize,
        tags: ColorTags,
        tone_map: Option<ToneMap>,
    },
    /// Anti-aliased rounded rectangle filled with a premultiplied linear colour.
    Shape { corner_radius: f32, color: [f32; 4] },
    /// Text node → glyph texture (WP-0F.02, MSDF atlas or Vello). `transform` maps box px to the
    /// pass's output px: text is rasterised at its final resolution (no resampling of glyphs).
    Text { node: TextNode, transform: [f32; 6] },
    /// MSDF text drawn directly over input 0 (the backdrop) with fixed-function premultiplied
    /// "over" and the layer `opacity`: no text intermediate, no clear, no full-frame composite.
    /// Used for normal-blend MSDF text layers without effects; `transform`: box px → canvas px.
    TextOver {
        node: TextNode,
        transform: [f32; 6],
        opacity: f32,
    },
    /// One direction of a separable Gaussian.
    Blur { horizontal: bool, radius: f32 },
    /// 3D LUT in `space`.
    Lut {
        lut: usize,
        space: Transfer,
        mix: f32,
    },
    /// Linear-light cross-dissolve of inputs `[from, to]`.
    Dissolve { progress: f32 },
    /// Places input 1 (layer) over input 0 (backdrop) through `inverse` (canvas → layer px).
    Composite {
        inverse: [f32; 6],
        opacity: f32,
        blend: BlendMode,
        layer_size: [f32; 2],
    },
    /// Output transform + quantisation into the readback buffer.
    Output { transfer: Transfer, bit_depth: u8 },
}

impl PassOp {
    /// Short op name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Fill { .. } => "fill",
            Self::Decode { .. } => "decode",
            Self::Shape { .. } => "shape",
            Self::Text { .. } => "text",
            Self::TextOver { .. } => "text-over",
            Self::Blur { .. } => "blur",
            Self::Lut { .. } => "lut",
            Self::Dissolve { .. } => "dissolve",
            Self::Composite { .. } => "composite",
            Self::Output { .. } => "output",
        }
    }
}

/// One node of the DAG.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub label: String,
    pub op: PassOp,
    pub inputs: Vec<ResId>,
    pub output: ResId,
    pub key: CacheKey,
    /// The output depends only on frame-invariant data (sources, params) and may be kept by the
    /// executor across frames.
    pub cacheable: bool,
}

/// Build statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub layers_culled: usize,
    pub transitions_collapsed: usize,
    pub transitions_layer_space: usize,
    pub passes_pruned: usize,
}

/// The compiled frame.
#[derive(Clone, Debug)]
pub struct RenderGraph {
    pub width: u32,
    pub height: u32,
    pub passes: Vec<Pass>,
    pub resources: Vec<ResourceDesc>,
    pub output: ResId,
    pub sources: Vec<Arc<SourceFrame>>,
    pub luts: Vec<Arc<Lut3d>>,
    pub stats: BuildStats,
}

impl RenderGraph {
    /// Resource → producing pass index. Errors on double producers.
    pub fn producers(&self) -> Result<Vec<Option<usize>>> {
        let mut prod = vec![None; self.resources.len()];
        for (i, p) in self.passes.iter().enumerate() {
            let slot = prod
                .get_mut(p.output)
                .ok_or_else(|| Error::InvalidGraph(format!("pass {i} writes unknown resource")))?;
            if slot.is_some() {
                return Err(Error::InvalidGraph(format!(
                    "resource {} written twice",
                    p.output
                )));
            }
            *slot = Some(i);
        }
        Ok(prod)
    }

    /// Deterministic topological order (Kahn, ties broken by pass index).
    pub fn topo_order(&self) -> Result<Vec<usize>> {
        let prod = self.producers()?;
        let n = self.passes.len();
        let mut indegree = vec![0usize; n];
        let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, p) in self.passes.iter().enumerate() {
            for &r in &p.inputs {
                let src = prod.get(r).copied().flatten().ok_or_else(|| {
                    Error::InvalidGraph(format!("pass `{}` reads unproduced resource {r}", p.label))
                })?;
                indegree[i] += 1;
                consumers[src].push(i);
            }
        }
        let mut ready: VecDeque<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = ready.pop_front() {
            order.push(i);
            for &c in &consumers[i] {
                indegree[c] -= 1;
                if indegree[c] == 0 {
                    ready.push_back(c);
                }
            }
        }
        if order.len() != n {
            return Err(Error::InvalidGraph("cycle detected".into()));
        }
        Ok(order)
    }

    /// Removes passes whose output does not reach [`Self::output`].
    pub fn prune(&mut self) -> Result<()> {
        let prod = self.producers()?;
        let mut live = vec![false; self.passes.len()];
        let mut stack = vec![self.output];
        while let Some(r) = stack.pop() {
            if let Some(Some(p)) = prod.get(r).copied()
                && !live[p]
            {
                live[p] = true;
                stack.extend(self.passes[p].inputs.iter().copied());
            }
        }
        let before = self.passes.len();
        let mut it = live.iter();
        self.passes.retain(|_| it.next().copied().unwrap_or(false));
        self.stats.passes_pruned += before - self.passes.len();
        Ok(())
    }

    /// For each resource, the position (in `order`) of its last reader.
    #[must_use]
    pub fn last_uses(&self, order: &[usize]) -> Vec<Option<usize>> {
        let mut last = vec![None; self.resources.len()];
        for (pos, &p) in order.iter().enumerate() {
            for &r in &self.passes[p].inputs {
                last[r] = Some(pos);
            }
        }
        last
    }

    /// Key of the output pass: identifies the frame's whole subgraph.
    #[must_use]
    pub fn output_key(&self) -> CacheKey {
        self.passes
            .iter()
            .find(|p| p.output == self.output)
            .map(|p| p.key)
            .unwrap_or_default()
    }

    /// Human-readable listing.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut s = String::new();
        for (i, p) in self.passes.iter().enumerate() {
            let r = self.resources[p.output];
            let _ = writeln!(
                s,
                "#{i:<3} {:<34} {:?} -> r{} {}x{} key={:?}{}",
                p.label,
                p.inputs,
                p.output,
                r.width,
                r.height,
                p.key,
                if p.cacheable { " [cacheable]" } else { "" }
            );
        }
        let _ = writeln!(s, "stats: {:?}", self.stats);
        s
    }
}

#[derive(Clone, Copy, Debug)]
struct Res {
    id: ResId,
}

struct Builder<'a> {
    provider: &'a dyn SourceProvider,
    g: RenderGraph,
    keys: Vec<CacheKey>,
    cacheable: Vec<bool>,
    source_ix: HashMap<u64, usize>,
    lut_ix: HashMap<String, usize>,
    frames: HashMap<(String, i64), Arc<SourceFrame>>,
}

/// Compiles `scene` into a render graph.
pub fn build(scene: &Scene, provider: &dyn SourceProvider) -> Result<RenderGraph> {
    scene.validate()?;
    let mut b = Builder {
        provider,
        g: RenderGraph {
            width: scene.width,
            height: scene.height,
            passes: Vec::new(),
            resources: Vec::new(),
            output: 0,
            sources: Vec::new(),
            luts: Vec::new(),
            stats: BuildStats::default(),
        },
        keys: Vec::new(),
        cacheable: Vec::new(),
        source_ix: HashMap::new(),
        lut_ix: HashMap::new(),
        frames: HashMap::new(),
    };
    let canvas = (scene.width, scene.height);
    let bg = b.pass(
        "background".into(),
        PassOp::Fill {
            color: scene.background,
        },
        vec![],
        canvas,
    );
    let composite = b.layers(bg, &scene.layers, canvas)?;
    let graded = b.effects(composite, &scene.effects, "sequence", canvas)?;
    let out = b.pass(
        "output".into(),
        PassOp::Output {
            transfer: scene.output.transfer,
            bit_depth: scene.output.bit_depth,
        },
        vec![graded.id],
        canvas,
    );
    b.g.output = out.id;
    b.g.prune()?;
    Ok(b.g)
}

impl Builder<'_> {
    fn hash_op(&self, op: &PassOp, h: &mut Fnv) {
        h.str(op.name());
        match op {
            PassOp::Fill { color } => {
                h.f32s(color);
            }
            PassOp::Decode {
                source,
                tags,
                tone_map,
            } => {
                h.u64(self.g.sources[*source].content_hash);
                h.u32(tags.matrix as u32)
                    .u32(tags.range as u32)
                    .u32(tags.transfer as u32)
                    .u32(tags.primaries as u32);
                if let Some(tm) = tone_map {
                    h.f32(tm.source_peak_nits)
                        .f32(tm.target_peak_nits)
                        .f32(tm.sdr_white_nits);
                }
            }
            PassOp::Shape {
                corner_radius,
                color,
            } => {
                h.f32(*corner_radius).f32s(color);
            }
            PassOp::Text { node, transform } => {
                hash_text(node, h);
                h.f32s(transform);
            }
            PassOp::TextOver {
                node,
                transform,
                opacity,
            } => {
                hash_text(node, h);
                h.f32s(transform).f32(*opacity);
            }
            PassOp::Blur { horizontal, radius } => {
                h.u32(u32::from(*horizontal)).f32(*radius);
            }
            PassOp::Lut { lut, space, mix } => {
                h.u64(self.g.luts[*lut].content_hash())
                    .u32(*space as u32)
                    .f32(*mix);
            }
            PassOp::Dissolve { progress } => {
                h.f32(*progress);
            }
            PassOp::Composite {
                inverse,
                opacity,
                blend,
                layer_size,
            } => {
                h.f32s(inverse)
                    .f32(*opacity)
                    .u32(blend.shader_id())
                    .f32s(layer_size);
            }
            PassOp::Output {
                transfer,
                bit_depth,
            } => {
                h.u32(*transfer as u32).u32(u32::from(*bit_depth));
            }
        }
    }

    fn pass(&mut self, label: String, op: PassOp, inputs: Vec<ResId>, size: (u32, u32)) -> Res {
        let mut h = Fnv::new();
        self.hash_op(&op, &mut h);
        h.u32(size.0).u32(size.1);
        for &i in &inputs {
            h.u64(self.keys[i].0);
        }
        let cacheable = match op {
            PassOp::Decode { .. } | PassOp::Shape { .. } | PassOp::Text { .. } => true,
            PassOp::Blur { .. } | PassOp::Lut { .. } => inputs.iter().all(|&i| self.cacheable[i]),
            _ => false,
        };
        let id = self.g.resources.len();
        self.g.resources.push(ResourceDesc {
            width: size.0,
            height: size.1,
        });
        self.keys.push(h.key());
        self.cacheable.push(cacheable);
        self.g.passes.push(Pass {
            label,
            op,
            inputs,
            output: id,
            key: h.key(),
            cacheable,
        });
        Res { id }
    }

    fn frame(&mut self, id: &str, frame: i64) -> Result<Arc<SourceFrame>> {
        let k = (id.to_string(), frame);
        if let Some(f) = self.frames.get(&k) {
            return Ok(Arc::clone(f));
        }
        let f = self.provider.frame(id, frame)?;
        self.frames.insert(k, Arc::clone(&f));
        Ok(f)
    }

    fn node_size(&mut self, node: &Node, canvas: (u32, u32)) -> Result<(u32, u32)> {
        let px = |v: f32| (v.ceil().max(1.0)) as u32;
        Ok(match node {
            Node::Media(m) => {
                let f = self.frame(&m.source, m.frame)?;
                (f.width, f.height)
            }
            Node::Shape(s) => (px(s.width), px(s.height)),
            Node::Group(g) => (g.width.max(1), g.height.max(1)),
            Node::Transition(_) => canvas,
            Node::Text(t) => (px(t.box_size[0]), px(t.box_size[1])),
        })
    }

    fn layers(&mut self, mut cur: Res, layers: &[Layer], canvas: (u32, u32)) -> Result<Res> {
        // Occlusion culling: everything below the topmost opaque full-canvas cover is invisible,
        // and compositing the cover itself is an exact copy, so its content becomes the base.
        let mut start = 0;
        for (i, layer) in layers.iter().enumerate().rev() {
            if self.is_opaque_cover(layer, canvas)? {
                self.g.stats.layers_culled += i;
                let content = self.node(&layer.content, &layer.id, canvas)?;
                cur = self.effects(content, &layer.effects, &layer.id, canvas)?;
                start = i + 1;
                break;
            }
        }
        for layer in &layers[start..] {
            cur = self.composite_layer(cur, layer, canvas)?;
        }
        Ok(cur)
    }

    /// A layer that replaces its backdrop exactly: normal blend, opacity 1, identity transform,
    /// canvas-sized, and content that is opaque everywhere (Y'CbCr media; effects that keep alpha).
    fn is_opaque_cover(&mut self, layer: &Layer, canvas: (u32, u32)) -> Result<bool> {
        let Node::Media(m) = &layer.content else {
            return Ok(false);
        };
        if layer.blend != BlendMode::Normal
            || layer.opacity < 1.0
            || layer.transform != crate::scene::Transform::default()
        {
            return Ok(false);
        }
        let f = self.frame(&m.source, m.frame)?;
        Ok((f.width, f.height) == canvas && matches!(f.pixels, PixelData::Yuv420 { .. }))
    }

    fn composite_layer(&mut self, backdrop: Res, layer: &Layer, canvas: (u32, u32)) -> Result<Res> {
        // `!(x > 0)` also culls NaN opacity.
        if layer.opacity.is_nan() || layer.opacity <= 0.0 {
            self.g.stats.layers_culled += 1;
            return Ok(backdrop);
        }
        // Same-geometry transitions dissolve in layer space: no canvas-sized sides, fills or
        // full-frame dissolve. Bilinear sampling and edge coverage are linear, so this equals the
        // canvas-space definition up to rounding.
        if let Node::Transition(t) = &layer.content
            && let Some((fwd, size)) = self.layer_space_transition(layer, t, canvas)?
        {
            let dims = [f64::from(size.0), f64::from(size.1)];
            let Some(inv) = invert_affine(&fwd).filter(|_| !offscreen(&fwd, dims, canvas)) else {
                self.g.stats.layers_culled += 1;
                return Ok(backdrop);
            };
            let a = self.node(&t.from.content, &t.from.id, canvas)?;
            let b = self.node(&t.to.content, &t.to.id, canvas)?;
            let p = t.progress.clamp(0.0, 1.0);
            let d = self.pass(
                format!("dissolve {} {p:.3} (layer space)", layer.id),
                PassOp::Dissolve { progress: p },
                vec![a.id, b.id],
                size,
            );
            self.g.stats.transitions_layer_space += 1;
            return Ok(self.place(backdrop, d, &inv, layer, size, canvas));
        }
        if let Node::Text(t) = &layer.content {
            return self.text_layer(backdrop, layer, t, canvas);
        }
        let (lw, lh) = self.node_size(&layer.content, canvas)?;
        let size = [f64::from(lw), f64::from(lh)];
        let fwd = layer.transform.forward(size);
        let Some(inv) = invert_affine(&fwd).filter(|_| !offscreen(&fwd, size, canvas)) else {
            self.g.stats.layers_culled += 1;
            return Ok(backdrop);
        };
        let content = self.node(&layer.content, &layer.id, canvas)?;
        let content = self.effects(content, &layer.effects, &layer.id, (lw, lh))?;
        Ok(self.place(backdrop, content, &inv, layer, (lw, lh), canvas))
    }

    /// Text layers are rasterised in canvas space: the text pass draws the glyphs through the
    /// full layer transform into a texture covering the transformed box's pixel bounds (clipped
    /// to the canvas), which the compositor then places with an integer translation (an exact
    /// copy). Glyphs are never resampled, so sub-pixel motion, rotation and scale stay crisp.
    /// Effects on a text layer therefore run in canvas pixels. Plain MSDF text layers skip the
    /// intermediate entirely (`TextOver`).
    fn text_layer(
        &mut self,
        backdrop: Res,
        layer: &Layer,
        t: &TextNode,
        canvas: (u32, u32),
    ) -> Result<Res> {
        let size = [
            f64::from(t.box_size[0].max(0.0)),
            f64::from(t.box_size[1].max(0.0)),
        ];
        let fwd = layer.transform.forward(size);
        let corners =
            [[0.0, 0.0], [size[0], 0.0], [0.0, size[1]], size].map(|p| apply_affine(&fwd, p));
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in corners {
            x0 = x0.min(c[0]);
            y0 = y0.min(c[1]);
            x1 = x1.max(c[0]);
            y1 = y1.max(c[1]);
        }
        let bx0 = x0.floor().max(0.0);
        let by0 = y0.floor().max(0.0);
        let bx1 = x1.ceil().min(f64::from(canvas.0));
        let by1 = y1.ceil().min(f64::from(canvas.1));
        if invert_affine(&fwd).is_none() || bx1 <= bx0 || by1 <= by0 || t.text.is_empty() {
            self.g.stats.layers_culled += 1;
            return Ok(backdrop);
        }
        // Fast path (WP-0F.01 finding 5a): normal-blend MSDF text without effects is drawn
        // straight onto the backdrop. Same result as text-then-composite up to f16 rounding of
        // the per-glyph blends.
        if layer.blend == BlendMode::Normal
            && layer.effects.is_empty()
            && t.renderer == crate::scene::TextRenderer::Msdf
        {
            return Ok(self.pass(
                format!("text-over {}", layer.id),
                PassOp::TextOver {
                    node: t.clone(),
                    transform: fwd.map(|v| v as f32),
                    opacity: layer.opacity.min(1.0),
                },
                vec![backdrop.id],
                canvas,
            ));
        }
        let dims = ((bx1 - bx0) as u32, (by1 - by0) as u32);
        let target = compose_affine(&[1.0, 0.0, -bx0, 0.0, 1.0, -by0], &fwd);
        let content = self.pass(
            format!("text {}", layer.id),
            PassOp::Text {
                node: t.clone(),
                transform: target.map(|v| v as f32),
            },
            vec![],
            dims,
        );
        let content = self.effects(content, &layer.effects, &layer.id, dims)?;
        Ok(self.place(
            backdrop,
            content,
            &[1.0, 0.0, -bx0, 0.0, 1.0, -by0],
            layer,
            dims,
            canvas,
        ))
    }

    fn place(
        &mut self,
        backdrop: Res,
        content: Res,
        inv: &Affine,
        layer: &Layer,
        size: (u32, u32),
        canvas: (u32, u32),
    ) -> Res {
        self.pass(
            format!("composite {} ({:?})", layer.id, layer.blend),
            PassOp::Composite {
                inverse: inv.map(|v| v as f32),
                opacity: layer.opacity.min(1.0),
                blend: layer.blend,
                layer_size: [size.0 as f32, size.1 as f32],
            },
            vec![backdrop.id, content.id],
            canvas,
        )
    }

    /// The combined (outer ∘ side) transform and size when a mid-progress transition can be
    /// dissolved in layer space: both sides plain (opacity 1, normal, no effects), same transform
    /// and size, and no effects on the transition layer itself.
    fn layer_space_transition(
        &mut self,
        layer: &Layer,
        t: &TransitionNode,
        canvas: (u32, u32),
    ) -> Result<Option<(Affine, (u32, u32))>> {
        let plain =
            |l: &Layer| l.opacity >= 1.0 && l.blend == BlendMode::Normal && l.effects.is_empty();
        let mid = t.progress > 0.0 && t.progress < 1.0;
        if !mid
            || !layer.effects.is_empty()
            || !plain(&t.from)
            || !plain(&t.to)
            || t.from.transform != t.to.transform
        {
            return Ok(None);
        }
        let sa = self.node_size(&t.from.content, canvas)?;
        if sa != self.node_size(&t.to.content, canvas)? {
            return Ok(None);
        }
        let inner = t.from.transform.forward([f64::from(sa.0), f64::from(sa.1)]);
        let outer = layer
            .transform
            .forward([f64::from(canvas.0), f64::from(canvas.1)]);
        Ok(Some((compose_affine(&outer, &inner), sa)))
    }

    fn node(&mut self, node: &Node, label: &str, canvas: (u32, u32)) -> Result<Res> {
        match node {
            Node::Media(m) => {
                let frame = self.frame(&m.source, m.frame)?;
                let tags = m.tags.unwrap_or(frame.tags);
                if matches!(frame.pixels, PixelData::Yuv420 { .. })
                    && tags.matrix == crate::scene::Matrix::Rgb
                {
                    return Err(Error::Unsupported(format!(
                        "{}: planar 4:2:0 data tagged as RGB",
                        m.source
                    )));
                }
                let tone_map =
                    (tags.transfer == Transfer::Pq).then(|| m.tone_map.unwrap_or_default());
                let source = *self.source_ix.entry(frame.content_hash).or_insert_with(|| {
                    self.g.sources.push(Arc::clone(&frame));
                    self.g.sources.len() - 1
                });
                Ok(self.pass(
                    format!("decode {}@{}", m.source, m.frame),
                    PassOp::Decode {
                        source,
                        tags,
                        tone_map,
                    },
                    vec![],
                    (frame.width, frame.height),
                ))
            }
            Node::Shape(s) => {
                let size = self.node_size(node, canvas)?;
                Ok(self.pass(
                    format!("shape {label}"),
                    PassOp::Shape {
                        corner_radius: s.corner_radius.max(0.0),
                        color: srgb_fill_to_linear_premul(s.fill_srgb),
                    },
                    vec![],
                    size,
                ))
            }
            Node::Text(t) => {
                // Intrinsic-size rendering (e.g. both sides of a layer-space transition).
                let size = self.node_size(node, canvas)?;
                Ok(self.pass(
                    format!("text {label}"),
                    PassOp::Text {
                        node: t.clone(),
                        transform: crate::text::IDENTITY,
                    },
                    vec![],
                    size,
                ))
            }
            Node::Group(g) => {
                let size = (g.width.max(1), g.height.max(1));
                let base = self.pass(
                    format!("group {label}"),
                    PassOp::Fill { color: [0.0; 4] },
                    vec![],
                    size,
                );
                self.layers(base, &g.layers, size)
            }
            Node::Transition(t) => {
                let p = if t.progress.is_nan() {
                    0.0
                } else {
                    t.progress.clamp(0.0, 1.0)
                };
                if p <= 0.0 {
                    self.g.stats.transitions_collapsed += 1;
                    return self.transition_side(&t.from, canvas);
                }
                if p >= 1.0 {
                    self.g.stats.transitions_collapsed += 1;
                    return self.transition_side(&t.to, canvas);
                }
                let a = self.transition_side(&t.from, canvas)?;
                let b = self.transition_side(&t.to, canvas)?;
                Ok(self.pass(
                    format!("dissolve {label} {p:.3}"),
                    PassOp::Dissolve { progress: p },
                    vec![a.id, b.id],
                    canvas,
                ))
            }
        }
    }

    fn transition_side(&mut self, layer: &Layer, canvas: (u32, u32)) -> Result<Res> {
        let base = self.pass(
            format!("canvas {}", layer.id),
            PassOp::Fill { color: [0.0; 4] },
            vec![],
            canvas,
        );
        self.composite_layer(base, layer, canvas)
    }

    fn effects(
        &mut self,
        mut cur: Res,
        effects: &[Effect],
        label: &str,
        size: (u32, u32),
    ) -> Result<Res> {
        for e in effects {
            match e {
                Effect::BlurLite { radius } => {
                    if *radius > 0.0 {
                        let r = radius.min(30.0);
                        let h = PassOp::Blur {
                            horizontal: true,
                            radius: r,
                        };
                        cur = self.pass(format!("blur-h {label}"), h, vec![cur.id], size);
                        let v = PassOp::Blur {
                            horizontal: false,
                            radius: r,
                        };
                        cur = self.pass(format!("blur-v {label}"), v, vec![cur.id], size);
                    }
                }
                Effect::Lut3d { lut, space, mix } => {
                    let idx = match self.lut_ix.get(lut) {
                        Some(&i) => i,
                        None => {
                            let l = self.provider.lut(lut)?;
                            self.g.luts.push(l);
                            let i = self.g.luts.len() - 1;
                            self.lut_ix.insert(lut.clone(), i);
                            i
                        }
                    };
                    cur = self.pass(
                        format!("lut {lut} {label}"),
                        PassOp::Lut {
                            lut: idx,
                            space: *space,
                            mix: mix.clamp(0.0, 1.0),
                        },
                        vec![cur.id],
                        size,
                    );
                }
            }
        }
        Ok(cur)
    }
}

/// Canonical hash of everything a text pass depends on.
fn hash_text(t: &TextNode, h: &mut Fnv) {
    h.str(&t.text)
        .str(&t.font)
        .f32(t.size_px)
        .f32s(&t.fill_srgb)
        .f32s(&t.box_size)
        .u32(t.align as u32)
        .f32(t.line_height)
        .u32(t.renderer as u32);
    match t.outline {
        Some(o) => h.u32(1).f32(o.width_px).f32s(&o.color_srgb),
        None => h.u32(0),
    };
    match t.shadow {
        Some(s) => h
            .u32(1)
            .f32s(&s.offset_px)
            .f32(s.blur_px)
            .f32s(&s.color_srgb),
        None => h.u32(0),
    };
    h.u64(t.spans.len() as u64);
    for s in &t.spans {
        h.u32(s.range[0])
            .u32(s.range[1])
            .u32(u32::from(s.per_glyph))
            .f32s(&s.offset_px)
            .f32s(&s.scale)
            .f32(s.rotation_deg)
            .f32(s.opacity);
        match s.fill_srgb {
            Some(c) => h.u32(1).f32s(&c),
            None => h.u32(0),
        };
        match s.highlight {
            Some(hl) => h.u32(1).f32s(&hl.color_srgb).f32(hl.progress),
            None => h.u32(0),
        };
    }
}

/// True when a `size` layer mapped by `fwd` cannot touch the canvas (½-px AA margin).
fn offscreen(fwd: &Affine, size: [f64; 2], canvas: (u32, u32)) -> bool {
    let corners = [[0.0, 0.0], [size[0], 0.0], [0.0, size[1]], size].map(|p| apply_affine(fwd, p));
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in corners {
        x0 = x0.min(c[0]);
        y0 = y0.min(c[1]);
        x1 = x1.max(c[0]);
        y1 = y1.max(c[1]);
    }
    x1 <= -0.5 || y1 <= -0.5 || x0 >= f64::from(canvas.0) + 0.5 || y0 >= f64::from(canvas.1) + 0.5
}

/// Gaussian weights `w[0..=n]` (centre first) for blur-lite, normalised so that
/// `w0 + 2·Σw[i>0] = 1`. σ = radius / 2, n = min(ceil(3σ), 15).
#[must_use]
pub fn blur_weights(radius: f32) -> Vec<f32> {
    let sigma = f64::from(radius.max(0.5)) / 2.0;
    let n = ((3.0 * sigma).ceil() as usize).clamp(1, 15);
    let raw: Vec<f64> = (0..=n)
        .map(|i| (-((i * i) as f64) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f64 = raw[0] + 2.0 * raw[1..].iter().sum::<f64>();
    raw.iter().map(|w| (w / total) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MediaNode, Transform};
    use crate::source::SyntheticSources;
    use crate::timeline::{demo_scene, text_scene};

    fn provider() -> SyntheticSources {
        SyntheticSources::new(320, 180).expect("sources")
    }

    fn assert_topological(g: &RenderGraph) {
        let order = g.topo_order().expect("acyclic");
        let prod = g.producers().expect("producers");
        let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &p)| (p, i)).collect();
        for (i, p) in g.passes.iter().enumerate() {
            for &r in &p.inputs {
                let src = prod[r].expect("produced");
                assert!(pos[&src] < pos[&i], "input produced before use");
            }
        }
        assert_eq!(g.passes[*order.last().expect("non-empty")].output, g.output);
    }

    #[test]
    fn demo_graph_shape() {
        let p = provider();
        let g = build(&demo_scene(320, 180, 150), &p).expect("build");
        assert_topological(&g);
        let count = |name: &str| g.passes.iter().filter(|p| p.op.name() == name).count();
        assert_eq!(count("decode"), 4, "camera, hdr10, phone601, chart");
        assert_eq!(count("dissolve"), 1);
        assert_eq!(count("blur"), 2);
        assert_eq!(count("lut"), 1);
        assert_eq!(count("composite"), 2, "pip, chart (bg is the base)");
        assert_eq!(
            count("fill"),
            0,
            "no transition canvases, background occluded"
        );
        assert_eq!(count("output"), 1);
        assert_eq!(g.stats.layers_culled, 0);
        assert_eq!(g.stats.transitions_layer_space, 1);
        let dissolve = g
            .passes
            .iter()
            .find(|p| p.op.name() == "dissolve")
            .expect("dissolve");
        assert_eq!(
            g.resources[dissolve.output],
            ResourceDesc {
                width: 160,
                height: 90
            }
        );
        assert!(
            !g.passes.iter().any(|p| p.label == "background"),
            "occluded by bg, pruned"
        );
        assert_eq!(g.stats.passes_pruned, 1);
    }

    #[test]
    fn transitions_with_different_side_geometry_dissolve_in_canvas_space() {
        let p = provider();
        let mut s = demo_scene(320, 180, 150);
        if let Node::Transition(t) = &mut s.layers[1].content {
            t.to.transform.rotation_deg += 5.0;
        }
        let g = build(&s, &p).expect("build");
        let count = |name: &str| g.passes.iter().filter(|p| p.op.name() == name).count();
        assert_eq!(g.stats.transitions_layer_space, 0);
        assert_eq!(count("composite"), 4, "two transition sides, pip, chart");
        assert_eq!(count("fill"), 2, "one transparent canvas per side");
        let dissolve = g
            .passes
            .iter()
            .find(|p| p.op.name() == "dissolve")
            .expect("dissolve");
        assert_eq!(
            g.resources[dissolve.output],
            ResourceDesc {
                width: 320,
                height: 180
            }
        );
    }

    #[test]
    fn opaque_full_canvas_layer_occludes_everything_below() {
        let p = provider();
        let mut s = demo_scene(320, 180, 0);
        // Move the chart to the bottom and put a second full-frame camera on top of it.
        let chart = s.layers.remove(2);
        s.layers.insert(0, chart);
        let g = build(&s, &p).expect("build");
        assert_eq!(
            g.stats.layers_culled, 1,
            "chart below the opaque bg is culled"
        );
        assert!(!g.passes.iter().any(|p| p.label.starts_with("decode chart")));
        // A cover with opacity < 1 does not occlude.
        s.layers[1].opacity = 0.99;
        let g = build(&s, &p).expect("build");
        assert_eq!(g.stats.layers_culled, 0);
        assert!(g.passes.iter().any(|p| p.label.starts_with("decode chart")));
    }

    #[test]
    fn transitions_collapse_at_ends() {
        let p = provider();
        let mut s = demo_scene(320, 180, 150);
        if let Node::Transition(t) = &mut s.layers[1].content {
            t.progress = 0.0;
        }
        let g = build(&s, &p).expect("build");
        assert_eq!(g.stats.transitions_collapsed, 1);
        assert!(g.passes.iter().all(|p| p.op.name() != "dissolve"));
        assert!(
            !g.passes
                .iter()
                .any(|p| p.label.starts_with("decode phone601"))
        );
    }

    #[test]
    fn culls_invisible_and_offscreen_layers() {
        let p = provider();
        let mut s = demo_scene(320, 180, 0);
        s.layers[2].opacity = 0.0;
        s.layers[1].transform.position = [5000.0, 5000.0];
        let g = build(&s, &p).expect("build");
        assert_eq!(g.stats.layers_culled, 2);
        assert_eq!(
            g.passes.iter().filter(|p| p.op.name() == "decode").count(),
            1
        );
    }

    #[test]
    fn cache_keys_are_stable_and_content_addressed() {
        let p = provider();
        let a = build(&demo_scene(320, 180, 10), &p).expect("a");
        let b = build(&demo_scene(320, 180, 10), &p).expect("b");
        let keys = |g: &RenderGraph| g.passes.iter().map(|p| p.key).collect::<Vec<_>>();
        assert_eq!(keys(&a), keys(&b), "same scene → same keys");

        let later = build(&demo_scene(320, 180, 11), &p).expect("later");
        let key_of = |g: &RenderGraph, label: &str| {
            g.passes
                .iter()
                .find(|p| p.label.starts_with(label))
                .map(|p| p.key)
                .expect(label)
        };
        assert_eq!(
            key_of(&a, "decode hdr10"),
            key_of(&later, "decode hdr10"),
            "static source"
        );
        assert_eq!(key_of(&a, "decode chart"), key_of(&later, "decode chart"));
        assert_ne!(
            key_of(&a, "decode camera709"),
            key_of(&later, "decode camera709")
        );
        assert_ne!(a.output_key(), later.output_key());
    }

    #[test]
    fn cacheability_propagates_through_effects_only() {
        let p = provider();
        let g = build(&demo_scene(320, 180, 0), &p).expect("build");
        for pass in &g.passes {
            match pass.op.name() {
                "decode" | "blur" => assert!(pass.cacheable, "{}", pass.label),
                "composite" | "output" | "fill" => assert!(!pass.cacheable, "{}", pass.label),
                _ => {}
            }
        }
        // The sequence LUT reads the composite, so it is not cacheable.
        let lut = g.passes.iter().find(|p| p.op.name() == "lut").expect("lut");
        assert!(!lut.cacheable);
    }

    #[test]
    fn prune_removes_dead_passes_and_detects_cycles() {
        let p = provider();
        let mut g = build(&demo_scene(320, 180, 0), &p).expect("build");
        let n = g.passes.len();
        let pruned_before = g.stats.passes_pruned;
        let dead = g.resources.len();
        g.resources.push(ResourceDesc {
            width: 1,
            height: 1,
        });
        g.passes.insert(
            0,
            Pass {
                label: "dead".into(),
                op: PassOp::Fill { color: [0.0; 4] },
                inputs: vec![],
                output: dead,
                key: CacheKey(1),
                cacheable: false,
            },
        );
        g.prune().expect("prune");
        assert_eq!(g.passes.len(), n);
        assert_eq!(g.stats.passes_pruned, pruned_before + 1);

        // Two passes reading each other's outputs.
        let mut cyc = g.clone();
        cyc.resources.push(ResourceDesc {
            width: 1,
            height: 1,
        });
        cyc.resources.push(ResourceDesc {
            width: 1,
            height: 1,
        });
        let (r1, r2) = (cyc.resources.len() - 2, cyc.resources.len() - 1);
        for (out, inp) in [(r1, r2), (r2, r1)] {
            cyc.passes.push(Pass {
                label: "cyc".into(),
                op: PassOp::Dissolve { progress: 0.5 },
                inputs: vec![inp],
                output: out,
                key: CacheKey(2),
                cacheable: false,
            });
        }
        assert!(matches!(cyc.topo_order(), Err(Error::InvalidGraph(_))));
    }

    fn with_caption(f: impl FnOnce(&mut Layer)) -> Scene {
        let mut s = text_scene(320, 180);
        f(&mut s.layers[3]);
        s
    }

    #[test]
    fn plain_msdf_text_is_drawn_over_the_backdrop_in_place() {
        let p = provider();
        let g = build(&text_scene(320, 180), &p).expect("build");
        let t = g
            .passes
            .iter()
            .find(|p| p.op.name() == "text-over")
            .expect("text-over pass");
        assert!(!t.cacheable);
        assert_eq!(t.inputs.len(), 1, "reads the backdrop");
        assert_eq!(
            g.resources[t.output],
            ResourceDesc {
                width: 320,
                height: 180
            }
        );
        let PassOp::TextOver { transform, .. } = &t.op else {
            panic!("text-over op")
        };
        assert_eq!(*transform, [1.0, 0.0, 80.0, 0.0, 1.0, -40.0]);
        assert!(
            !g.passes
                .iter()
                .any(|p| p.label.starts_with("composite caption"))
        );
    }

    #[test]
    fn other_text_layers_render_to_a_canvas_space_intermediate() {
        let p = provider();
        for s in [
            with_caption(|l| l.blend = BlendMode::Screen),
            with_caption(|l| l.effects.push(Effect::BlurLite { radius: 2.0 })),
            with_caption(|l| {
                if let Node::Text(t) = &mut l.content {
                    t.renderer = crate::scene::TextRenderer::Vello;
                }
            }),
        ] {
            let g = build(&s, &p).expect("build");
            let t = g
                .passes
                .iter()
                .find(|p| p.op.name() == "text")
                .expect("text pass");
            assert!(t.cacheable);
            // Box 520×110 at (80, −40) on a 320×180 canvas → pixel bounds clipped to the canvas.
            assert_eq!(
                g.resources[t.output],
                ResourceDesc {
                    width: 240,
                    height: 70
                }
            );
            let PassOp::Text { transform, .. } = &t.op else {
                panic!("text op")
            };
            assert_eq!(*transform, [1.0, 0.0, 0.0, 0.0, 1.0, -40.0]);
            // Placed with an integer translation (exact copy).
            let c = g
                .passes
                .iter()
                .find(|p| p.label.starts_with("composite caption"))
                .expect("composite");
            let PassOp::Composite { inverse, .. } = c.op else {
                panic!("composite op")
            };
            assert_eq!(inverse, [1.0, 0.0, -80.0, 0.0, 1.0, 0.0]);
        }
    }

    #[test]
    fn animated_text_changes_the_key_static_text_does_not() {
        let p = provider();
        let key = |s: &Scene| {
            let g = build(s, &p).expect("build");
            g.passes
                .iter()
                .find(|p| p.op.name().starts_with("text"))
                .map(|p| p.key)
                .expect("text")
        };
        let a = text_scene(320, 180);
        let mut b = a.clone();
        assert_eq!(key(&a), key(&b));
        if let Node::Text(t) = &mut b.layers[3].content {
            t.spans.push(crate::scene::TextSpan::new([0, 3]));
        }
        assert_ne!(key(&a), key(&b));
    }

    #[test]
    fn unknown_source_is_an_error() {
        let p = provider();
        let mut s = demo_scene(320, 180, 0);
        s.layers[0].content = Node::Media(MediaNode {
            source: "nope".into(),
            frame: 0,
            tags: None,
            tone_map: None,
        });
        s.layers[0].transform = Transform::default();
        assert!(matches!(build(&s, &p), Err(Error::UnknownSource(_))));
    }

    #[test]
    fn blur_weights_normalised() {
        for r in [0.5, 1.0, 3.0, 12.0, 30.0] {
            let w = blur_weights(r);
            let total: f32 = w[0] + 2.0 * w[1..].iter().sum::<f32>();
            assert!((total - 1.0).abs() < 1e-5);
            assert!(w.len() <= 16);
        }
    }
}
