// SPDX-License-Identifier: GPL-3.0-or-later
//! The render graph (arch §8: "cull · order · allocate · cache keys"): a DAG of passes compiled
//! from a [`SceneDescription`] for one output target.
//!
//! * **Passes** read 0..n resources and write exactly one. A source frame lowers to a `Decode`
//!   pass producing a premultiplied, linear-light, Rec.709 `Rgba16Float` texture of the frame's
//!   size (plus the mip levels its layers need); a `Background` pass starts the canvas; each
//!   visible layer is a `Composite` pass placing a decoded texture over the canvas with its
//!   transform, crop, opacity and blend mode; `Output` encodes the canvas for CPU readback.
//!   Composites are written as new resources (a pure DAG); the executor aliases a backdrop to
//!   the composite that is its last reader, so compositing happens in place.
//! * **Cache keys** are Merkle hashes: `key = H(op, params, size, input keys)`. A still image's
//!   decode has the same key on every frame, so the executor reuses its texture; the output key
//!   identifies the whole frame, i.e. the `(subgraph hash, t)` frame-cache key of §8.
//! * **Culling** happens while building, before any frame is requested where possible: zero
//!   opacity, degenerate transforms, empty crops, layers entirely off the output, frames the
//!   source cannot provide (recorded in [`RenderGraph::missing`]), and **occlusion**: everything
//!   below an opaque (Y'CbCr), normal-blend, opacity-1 layer covering the whole output is never
//!   decoded. Compositing over such a cover is exact (`src + dst·0`), so culling never changes
//!   a pixel. Two layers showing the same frame share one decode.
//! * **Order** is a deterministic Kahn topological sort; the executor derives texture lifetimes
//!   (last readers) from it. Unreachable passes are pruned.
//!
//! Blend modes: only `Normal` exists in schema v1; the `Composite` op carries the mode so the P2
//! core set (multiply, screen, …) slots in without changing the graph shape (arch §8).

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use cutweave_model::ItemId;

use crate::color::{
    InputColor, InputTransform, OutputTransfer, ToneMap, merge_tags, srgb8_to_linear_premul,
};
use crate::frame::{FrameError, FrameSource, VideoFrame};
use crate::geometry::{LayerGeometry, mip_levels_for};
use crate::hash::{CacheKey, Fnv};
use crate::scene::{BlendMode, SceneDescription};

/// What a render produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Target {
    /// Read back 8-bit RGBA (straight alpha) encoded with `transfer`: preview frames, agent
    /// snapshots, PNG export.
    Rgba8Cpu {
        /// Output width in pixels.
        width: u32,
        /// Output height in pixels.
        height: u32,
        /// Output encoding.
        transfer: OutputTransfer,
    },
    /// Keep the composited canvas on the GPU: premultiplied linear-light `Rgba16Float` (for GPU
    /// consumers: NV12 conversion, encoders, further passes).
    Texture {
        /// Output width in pixels.
        width: u32,
        /// Output height in pixels.
        height: u32,
    },
}

impl Target {
    /// An sRGB RGBA8 readback of `width × height` (the preview default).
    #[must_use]
    pub const fn rgba8(width: u32, height: u32) -> Target {
        Target::Rgba8Cpu {
            width,
            height,
            transfer: OutputTransfer::Srgb,
        }
    }

    /// The output size.
    #[must_use]
    pub const fn size(&self) -> (u32, u32) {
        match *self {
            Target::Rgba8Cpu { width, height, .. } | Target::Texture { width, height } => {
                (width, height)
            }
        }
    }
}

/// Settings that change pixels (and are therefore part of every cache key they affect).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GraphOptions {
    /// HDR → SDR tone map.
    pub tone_map: ToneMap,
}

/// Why a graph could not be built or executed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    /// The target or the canvas has no pixels.
    #[error("empty {what}: {width}x{height}")]
    Empty {
        /// `"output"` or `"canvas"`.
        what: &'static str,
        /// Width.
        width: u32,
        /// Height.
        height: u32,
    },
    /// The graph is malformed (a bug in a builder).
    #[error("invalid render graph: {0}")]
    Invalid(String),
}

/// Index into [`RenderGraph::resources`].
pub type ResId = usize;

/// A 2D texture (or, for the output pass, the readback buffer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Mip levels.
    pub mip_levels: u32,
}

/// Upload + input colour transform of one source frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeOp {
    /// Index into [`RenderGraph::frames`].
    pub frame: usize,
    /// The input transform constants.
    pub transform: InputTransform,
    /// Mip levels to generate (1 = none).
    pub mip_levels: u32,
}

/// One layer placed over the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompositeOp {
    /// The item the layer comes from.
    pub item: ItemId,
    /// Placement and sampling.
    pub geometry: LayerGeometry,
    /// Opacity in `(0, 1]`.
    pub opacity: f64,
    /// Blend mode.
    pub blend: BlendMode,
    /// Mip level of detail (0 = no minification).
    pub lod: f64,
    /// Output-pixel rectangle to rasterise `[x0, y0, x1, y1)`.
    pub rect: [u32; 4],
    /// The layer covers every output pixel completely ([`LayerGeometry::covers`]): coverage is
    /// exactly 1, not evaluated per pixel.
    pub full_cover: bool,
}

/// What a pass does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PassOp {
    /// Start a canvas filled with a premultiplied linear colour.
    Background {
        /// Premultiplied linear RGBA.
        color: [f64; 4],
    },
    /// Upload a frame and convert it to premultiplied linear Rec.709.
    Decode(DecodeOp),
    /// Place input 1 (a decoded frame) over input 0 (the canvas).
    Composite(CompositeOp),
    /// Encode input 0 for readback.
    Output {
        /// Output encoding.
        transfer: OutputTransfer,
    },
}

impl PassOp {
    /// Short op name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            PassOp::Background { .. } => "background",
            PassOp::Decode(_) => "decode",
            PassOp::Composite(_) => "composite",
            PassOp::Output { .. } => "output",
        }
    }
}

/// One node of the DAG.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    /// The operation.
    pub op: PassOp,
    /// Resources read.
    pub inputs: Vec<ResId>,
    /// Resource written.
    pub output: ResId,
    /// Merkle key of the output.
    pub key: CacheKey,
    /// The output depends only on frame content and parameters and may be kept by the executor
    /// across frames.
    pub cacheable: bool,
}

/// Build statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    /// Layers composited.
    pub layers_visible: usize,
    /// Layers dropped for zero opacity, degenerate geometry or being off the output.
    pub layers_culled: usize,
    /// Layers hidden under an opaque full-output layer (never decoded).
    pub layers_occluded: usize,
    /// Decode passes shared by more than one layer.
    pub decodes_shared: usize,
    /// Passes removed as unreachable from the output.
    pub passes_pruned: usize,
}

/// A compiled frame.
#[derive(Clone, Debug)]
pub struct RenderGraph {
    /// What the graph renders to.
    pub target: Target,
    /// The passes (in build order; execute in [`RenderGraph::topo_order`]).
    pub passes: Vec<Pass>,
    /// Resource descriptions.
    pub resources: Vec<ResourceDesc>,
    /// The resource holding the result.
    pub output: ResId,
    /// Frames referenced by decode passes.
    pub frames: Vec<Arc<VideoFrame>>,
    /// Layers skipped because their frame was unavailable.
    pub missing: Vec<(ItemId, FrameError)>,
    /// Build statistics.
    pub stats: BuildStats,
}

impl RenderGraph {
    /// Resource → index of the pass producing it. Errors on a resource written twice.
    pub fn producers(&self) -> Result<Vec<Option<usize>>, GraphError> {
        let mut prod = vec![None; self.resources.len()];
        for (i, p) in self.passes.iter().enumerate() {
            let slot = prod
                .get_mut(p.output)
                .ok_or_else(|| GraphError::Invalid(format!("pass {i} writes unknown resource")))?;
            if slot.is_some() {
                return Err(GraphError::Invalid(format!(
                    "resource {} written twice",
                    p.output
                )));
            }
            *slot = Some(i);
        }
        Ok(prod)
    }

    /// Deterministic topological order (Kahn, ties broken by pass index).
    pub fn topo_order(&self) -> Result<Vec<usize>, GraphError> {
        let prod = self.producers()?;
        let n = self.passes.len();
        let mut indegree = vec![0usize; n];
        let mut consumers: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, p) in self.passes.iter().enumerate() {
            for &r in &p.inputs {
                let src = prod.get(r).copied().flatten().ok_or_else(|| {
                    GraphError::Invalid(format!(
                        "{} pass {i} reads unproduced resource {r}",
                        p.op.name()
                    ))
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
            return Err(GraphError::Invalid("cycle detected".into()));
        }
        Ok(order)
    }

    /// Removes passes whose output does not reach [`RenderGraph::output`].
    pub fn prune(&mut self) -> Result<(), GraphError> {
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

    /// Key of the pass producing the output: identifies the frame's whole subgraph.
    #[must_use]
    pub fn output_key(&self) -> CacheKey {
        self.passes
            .iter()
            .find(|p| p.output == self.output)
            .map_or(CacheKey::default(), |p| p.key)
    }

    /// Human-readable listing (debugging, `--graph` dumps).
    #[must_use]
    pub fn describe(&self) -> String {
        let mut s = String::new();
        for (i, p) in self.passes.iter().enumerate() {
            let r = self.resources[p.output];
            let what = match &p.op {
                PassOp::Composite(c) => format!("composite {}", c.item),
                PassOp::Decode(d) => format!(
                    "decode frame {:016x} ({} mips)",
                    self.frames[d.frame].id.0, d.mip_levels
                ),
                op => op.name().to_string(),
            };
            let _ = writeln!(
                s,
                "#{i:<3} {what:<48} {:?} -> r{} {}x{} key={}{}",
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

/// A layer that survived culling.
struct Visible {
    index: usize,
    frame: Arc<VideoFrame>,
    geometry: LayerGeometry,
    transform: InputTransform,
}

/// Compiles `scene` for `target`, pulling frames from `source`.
pub fn build(
    scene: &SceneDescription,
    target: Target,
    source: &dyn FrameSource,
    options: &GraphOptions,
) -> Result<RenderGraph, GraphError> {
    let output = target.size();
    if output.0 == 0 || output.1 == 0 {
        return Err(GraphError::Empty {
            what: "output",
            width: output.0,
            height: output.1,
        });
    }
    let canvas = (scene.width, scene.height);
    if canvas.0 == 0 || canvas.1 == 0 {
        return Err(GraphError::Empty {
            what: "canvas",
            width: canvas.0,
            height: canvas.1,
        });
    }
    let mut g = RenderGraph {
        target,
        passes: Vec::new(),
        resources: Vec::new(),
        output: 0,
        frames: Vec::new(),
        missing: Vec::new(),
        stats: BuildStats::default(),
    };

    let visible = cull(scene, output, source, options, &mut g);
    let (decode_of, decodes) = plan_decodes(&visible, &mut g);

    let mut keys: Vec<CacheKey> = Vec::new();
    let mut decode_res: Vec<ResId> = Vec::with_capacity(decodes.len());
    for &(frame, transform, mip_levels) in &decodes {
        let f = &g.frames[frame];
        let size = ResourceDesc {
            width: f.width,
            height: f.height,
            mip_levels,
        };
        let op = PassOp::Decode(DecodeOp {
            frame,
            transform,
            mip_levels,
        });
        decode_res.push(push_pass(&mut g, &mut keys, op, Vec::new(), size));
    }

    let canvas_desc = ResourceDesc {
        width: output.0,
        height: output.1,
        mip_levels: 1,
    };
    let bg = srgb8_to_linear_premul(scene.background);
    let mut cur = push_pass(
        &mut g,
        &mut keys,
        PassOp::Background { color: bg },
        Vec::new(),
        canvas_desc,
    );
    for (v, &slot) in visible.iter().zip(&decode_of) {
        let layer = &scene.layers[v.index];
        let op = PassOp::Composite(CompositeOp {
            item: layer.item,
            geometry: v.geometry,
            opacity: layer.opacity.min(1.0),
            blend: layer.blend,
            lod: v.geometry.lod(),
            rect: v.geometry.pixel_rect(output),
            full_cover: v.geometry.covers(output),
        });
        cur = push_pass(
            &mut g,
            &mut keys,
            op,
            vec![cur, decode_res[slot]],
            canvas_desc,
        );
    }
    if let Target::Rgba8Cpu { transfer, .. } = target {
        cur = push_pass(
            &mut g,
            &mut keys,
            PassOp::Output { transfer },
            vec![cur],
            canvas_desc,
        );
    }
    g.output = cur;
    g.prune()?;
    Ok(g)
}

/// Culls layers top-down, so that nothing under an opaque cover is ever requested from the
/// source; returns the visible layers bottom-up.
fn cull(
    scene: &SceneDescription,
    output: (u32, u32),
    source: &dyn FrameSource,
    options: &GraphOptions,
    g: &mut RenderGraph,
) -> Vec<Visible> {
    let canvas = (scene.width, scene.height);
    let mut visible = Vec::new();
    for (index, layer) in scene.layers.iter().enumerate().rev() {
        if layer.opacity.is_nan() || layer.opacity <= 0.0 {
            g.stats.layers_culled += 1;
            continue;
        }
        let frame = match source
            .frame(&layer.request)
            .and_then(|f| f.validate().map(|()| f))
        {
            Ok(f) => f,
            Err(e) => {
                g.missing.push((layer.item, e));
                continue;
            }
        };
        let Some(geometry) = LayerGeometry::new(layer, canvas, output, (frame.width, frame.height))
            .filter(|geo| !geo.offscreen(output))
        else {
            g.stats.layers_culled += 1;
            continue;
        };
        let height = layer.request.source_size.map_or(frame.height, |s| s.1);
        let color = InputColor::resolve(
            &merge_tags(&frame.color, &layer.request.color),
            frame.format.is_yuv(),
            height,
        );
        let transform = InputTransform::new(&color, frame.format.bit_depth(), &options.tone_map);
        let cover = layer.blend == BlendMode::Normal
            && layer.opacity >= 1.0
            && frame.format.is_opaque()
            && geometry.covers(output);
        visible.push(Visible {
            index,
            frame,
            geometry,
            transform,
        });
        if cover {
            g.stats.layers_occluded += index;
            break;
        }
    }
    visible.reverse();
    g.missing.reverse();
    g.stats.layers_visible = visible.len();
    visible
}

/// One decode per distinct (frame, input transform), with the most mips any of its layers
/// needs. Returns each visible layer's decode slot and the decodes `(frame, transform, mips)`.
#[allow(clippy::type_complexity)]
fn plan_decodes(
    visible: &[Visible],
    g: &mut RenderGraph,
) -> (Vec<usize>, Vec<(usize, InputTransform, u32)>) {
    let mut decode_of: Vec<usize> = Vec::with_capacity(visible.len());
    let mut decodes: Vec<(usize, InputTransform, u32)> = Vec::new();
    let mut by_key: HashMap<u64, usize> = HashMap::new();
    for v in visible {
        let mut h = Fnv::new();
        h.u64(v.frame.id.0);
        hash_transform(&v.transform, &mut h);
        let mips = mip_levels_for(v.geometry.lod(), v.frame.width, v.frame.height);
        let slot = *by_key.entry(h.finish()).or_insert_with(|| {
            let frame = g.frames.len();
            g.frames.push(Arc::clone(&v.frame));
            decodes.push((frame, v.transform, 1));
            decodes.len() - 1
        });
        if decode_of.contains(&slot) {
            g.stats.decodes_shared += 1;
        }
        decodes[slot].2 = decodes[slot].2.max(mips);
        decode_of.push(slot);
    }
    (decode_of, decodes)
}

// `op` is moved into the graph, so it is taken by value.
#[allow(clippy::large_types_passed_by_value)]
fn push_pass(
    g: &mut RenderGraph,
    keys: &mut Vec<CacheKey>,
    op: PassOp,
    inputs: Vec<ResId>,
    size: ResourceDesc,
) -> ResId {
    let mut h = Fnv::new();
    h.str(op.name());
    match &op {
        PassOp::Background { color } => {
            h.f64s(color);
        }
        PassOp::Decode(d) => {
            let f = &g.frames[d.frame];
            h.u64(f.id.0)
                .str(&format!("{:?}", f.format))
                .f64s(&f.chroma_siting.offset())
                .u32(d.mip_levels);
            hash_transform(&d.transform, &mut h);
        }
        PassOp::Composite(c) => {
            let geo = &c.geometry;
            h.f64s(&geo.from_output)
                .f64s(&geo.to_texel)
                .f64s(&geo.visible)
                .f64s(&geo.edge_scale)
                .f64(c.opacity)
                .f64(c.lod)
                .u32(u32::from(c.full_cover))
                .str(&format!("{:?}", c.blend));
            for v in c.rect {
                h.u32(v);
            }
        }
        PassOp::Output { transfer } => {
            h.u32(transfer.transfer().shader_id());
        }
    }
    h.u32(size.width).u32(size.height).u32(size.mip_levels);
    for &i in &inputs {
        h.u64(keys[i].0);
    }
    let id = g.resources.len();
    g.resources.push(size);
    keys.push(h.key());
    g.passes.push(Pass {
        cacheable: matches!(op, PassOp::Decode(_)),
        op,
        inputs,
        output: id,
        key: h.key(),
    });
    id
}

fn hash_transform(t: &InputTransform, h: &mut Fnv) {
    match t.kr_kb {
        Some((kr, kb)) => h.u32(1).f64(kr).f64(kb),
        None => h.u32(0),
    };
    h.f64(t.range.y_off)
        .f64(t.range.y_scale)
        .f64(t.range.c_off)
        .f64(t.range.c_scale)
        .u32(t.transfer.shader_id())
        .f64(t.linear_scale);
    match t.hlg_ootf {
        Some((luma, g)) => h.u32(1).f64s(&luma).f64(g),
        None => h.u32(0),
    };
    for row in &t.primaries {
        h.f64s(row);
    }
    match &t.tone_map {
        Some(e) => h
            .u32(1)
            .f64(e.src_min_pq)
            .f64(e.src_max_pq)
            .f64(e.min_lum)
            .f64(e.max_lum)
            .f64(e.ks)
            .f64(e.sdr_white_nits),
        None => h.u32(0),
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::many_single_char_names)]

    use std::cell::RefCell;

    use cutweave_model::{Color, ColorTags, FrameRate, MediaId, SequenceId, Time, TrackId, Vec2};

    use super::*;
    use crate::frame::PixelFormat;
    use crate::scene::{Crop, FrameRequest, Layer, LayerTransform};
    use crate::testcard::{Alpha, Pattern, SyntheticSource, TestCard};

    fn layer(n: u128, media: u128, size: (u32, u32)) -> Layer {
        Layer {
            item: ItemId::from_u128(n),
            track: TrackId::from_u128(n),
            request: FrameRequest {
                media: MediaId::from_u128(media),
                stream: Some(0),
                source_time: Time::ZERO,
                source_size: Some(size),
                rotation: 0,
                color: ColorTags::default(),
                still: false,
            },
            transform: LayerTransform {
                position: Vec2::ZERO,
                scale: Vec2::ONE,
                rotation_deg: 0.0,
                anchor: Vec2::new(0.5, 0.5),
            },
            opacity: 1.0,
            blend: BlendMode::Normal,
            crop: Crop::default(),
            effects: Vec::new(),
        }
    }

    fn scene(layers: Vec<Layer>) -> SceneDescription {
        SceneDescription {
            sequence: SequenceId::from_u128(1),
            time: Time::ZERO,
            width: 64,
            height: 36,
            frame_rate: FrameRate::FPS_24,
            background: Color::rgb(10, 20, 30),
            layers,
            audio: Vec::new(),
        }
    }

    fn card(format: PixelFormat) -> TestCard {
        TestCard::new(64, 36, Pattern::ColorBars, format, ColorTags::default())
    }

    fn source() -> SyntheticSource {
        SyntheticSource::new()
            .with(MediaId::from_u128(10), card(PixelFormat::Nv12))
            .with(MediaId::from_u128(11), card(PixelFormat::Yuv420p))
            .with(
                MediaId::from_u128(12),
                TestCard {
                    alpha: Alpha::RampX,
                    ..card(PixelFormat::Rgba8)
                },
            )
    }

    /// Counts the frames requested from an inner source.
    struct Counting<'a>(&'a dyn FrameSource, RefCell<Vec<MediaId>>);

    impl FrameSource for Counting<'_> {
        fn frame(&self, request: &FrameRequest) -> Result<Arc<VideoFrame>, FrameError> {
            self.1.borrow_mut().push(request.media);
            self.0.frame(request)
        }
    }

    fn ops(g: &RenderGraph) -> Vec<&'static str> {
        let order = g.topo_order().unwrap();
        order.iter().map(|&i| g.passes[i].op.name()).collect()
    }

    fn assert_topological(g: &RenderGraph) {
        let order = g.topo_order().expect("acyclic");
        let prod = g.producers().expect("producers");
        let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &p)| (p, i)).collect();
        for (i, p) in g.passes.iter().enumerate() {
            for &r in &p.inputs {
                assert!(
                    pos[&prod[r].unwrap()] < pos[&i],
                    "input produced before use"
                );
            }
        }
        assert_eq!(g.passes[*order.last().unwrap()].output, g.output);
    }

    #[test]
    fn single_layer_graph_shape_and_stable_keys() {
        let s = scene(vec![layer(1, 10, (64, 36))]);
        let g = build(
            &s,
            Target::rgba8(64, 36),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert_topological(&g);
        assert_eq!(ops(&g), ["decode", "background", "composite", "output"]);
        assert!(
            g.passes
                .iter()
                .filter(|p| p.cacheable)
                .all(|p| p.op.name() == "decode")
        );
        let again = build(
            &s,
            Target::rgba8(64, 36),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert_eq!(g.output_key(), again.output_key());
        // Another encoding, size or tone map changes the frame key.
        let bt709 = Target::Rgba8Cpu {
            width: 64,
            height: 36,
            transfer: OutputTransfer::Bt709,
        };
        let other = build(&s, bt709, &source(), &GraphOptions::default()).unwrap();
        assert_ne!(g.output_key(), other.output_key());
        assert_eq!(
            g.passes[0].key, other.passes[0].key,
            "decode key is target-independent"
        );
        let half = build(
            &s,
            Target::rgba8(32, 18),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert_ne!(g.output_key(), half.output_key());
        assert!(g.describe().contains("composite"));
    }

    #[test]
    fn texture_targets_have_no_output_pass() {
        let s = scene(vec![layer(1, 10, (64, 36))]);
        let t = Target::Texture {
            width: 64,
            height: 36,
        };
        let g = build(&s, t, &source(), &GraphOptions::default()).unwrap();
        assert_eq!(ops(&g), ["decode", "background", "composite"]);
        let empty = build(&scene(vec![]), t, &source(), &GraphOptions::default()).unwrap();
        assert_eq!(ops(&empty), ["background"]);
    }

    #[test]
    fn invisible_and_missing_layers_are_culled() {
        let mut faded = layer(1, 10, (64, 36));
        faded.opacity = 0.0;
        let mut nan = layer(2, 10, (64, 36));
        nan.opacity = f64::NAN;
        let mut away = layer(3, 11, (64, 36));
        away.transform.position = Vec2::new(3.0, 0.0);
        let mut flat = layer(4, 11, (64, 36));
        flat.transform.scale = Vec2::new(1.0, 0.0);
        let offline = layer(5, 99, (64, 36));
        let s = scene(vec![faded, nan, away, flat, offline]);
        let g = build(
            &s,
            Target::rgba8(64, 36),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert_eq!(ops(&g), ["background", "output"]);
        assert_eq!(g.stats.layers_culled, 4);
        assert_eq!(g.missing.len(), 1);
        assert_eq!(g.missing[0].0, ItemId::from_u128(5));
    }

    #[test]
    fn opaque_full_frame_layers_occlude_everything_below() {
        let bottom = layer(1, 11, (64, 36));
        let middle = layer(2, 12, (64, 36));
        let top = layer(3, 10, (64, 36));
        let inner = source();
        let counting = Counting(&inner, RefCell::new(Vec::new()));
        let s = scene(vec![bottom.clone(), middle.clone(), top.clone()]);
        let g = build(
            &s,
            Target::rgba8(64, 36),
            &counting,
            &GraphOptions::default(),
        )
        .unwrap();
        assert_eq!(ops(&g), ["decode", "background", "composite", "output"]);
        assert_eq!(g.stats.layers_occluded, 2);
        assert_eq!(
            *counting.1.borrow(),
            [MediaId::from_u128(10)],
            "nothing below is decoded"
        );

        // An RGBA top layer, a smaller one, or one at partial opacity occludes nothing.
        let mut half = top.clone();
        half.opacity = 0.5;
        let mut small = top.clone();
        small.transform.scale = Vec2::new(0.9, 0.9);
        for top in [middle, half, small] {
            let s = scene(vec![bottom.clone(), top]);
            let g = build(
                &s,
                Target::rgba8(64, 36),
                &source(),
                &GraphOptions::default(),
            )
            .unwrap();
            assert_eq!(g.stats.layers_visible, 2);
            assert_eq!(g.stats.layers_occluded, 0);
        }
    }

    #[test]
    fn layers_showing_the_same_frame_share_one_decode() {
        let mut a = layer(1, 11, (64, 36));
        a.transform.scale = Vec2::new(0.5, 0.5);
        a.transform.position = Vec2::new(-0.25, 0.0);
        let mut b = a.clone();
        b.item = ItemId::from_u128(2);
        b.transform.position = Vec2::new(0.25, 0.0);
        let s = scene(vec![a, b]);
        let g = build(
            &s,
            Target::rgba8(64, 36),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        assert_eq!(
            ops(&g),
            ["decode", "background", "composite", "composite", "output"]
        );
        assert_eq!(g.stats.decodes_shared, 1);
        assert_eq!(g.frames.len(), 1);
        // Half size → LOD 1 → the decode carries a mip chain for trilinear sampling.
        let PassOp::Decode(d) = g.passes[0].op else {
            panic!("decode first")
        };
        assert_eq!(d.mip_levels, 3);
        assert_eq!(g.resources[g.passes[0].output].mip_levels, 3);
    }

    #[test]
    fn decode_keys_follow_content_not_time() {
        // A still keeps its decode key whatever the evaluated time; an animated card does not.
        let still = scene(vec![layer(1, 10, (64, 36))]);
        let mut later = still.clone();
        later.time = Time::from_secs(5).unwrap();
        later.layers[0].request.source_time = Time::from_secs(5).unwrap();
        let src = source();
        let k = |s: &SceneDescription, src: &dyn FrameSource| {
            build(s, Target::rgba8(64, 36), src, &GraphOptions::default())
                .unwrap()
                .passes[0]
                .key
        };
        assert_eq!(k(&still, &src), k(&later, &src));
        let animated = SyntheticSource::new().with(
            MediaId::from_u128(10),
            TestCard {
                animated: true,
                ..card(PixelFormat::Nv12)
            },
        );
        assert_ne!(k(&still, &animated), k(&later, &animated));
    }

    #[test]
    fn pruning_and_cycles() {
        let s = scene(vec![layer(1, 10, (64, 36))]);
        let mut g = build(
            &s,
            Target::rgba8(64, 36),
            &source(),
            &GraphOptions::default(),
        )
        .unwrap();
        // A dangling pass is pruned.
        let dangling = g.passes[1].clone();
        g.resources.push(g.resources[dangling.output]);
        g.passes.push(Pass {
            output: g.resources.len() - 1,
            ..dangling
        });
        g.prune().unwrap();
        assert_eq!(g.passes.len(), 4);
        assert_eq!(g.stats.passes_pruned, 1);
        // A cycle is detected.
        let last = g.passes[g.passes.len() - 1].output;
        g.passes[0].inputs.push(last);
        assert!(matches!(g.topo_order(), Err(GraphError::Invalid(_))));
    }

    #[test]
    fn empty_targets_are_rejected() {
        let s = scene(vec![]);
        assert!(matches!(
            build(
                &s,
                Target::rgba8(0, 10),
                &source(),
                &GraphOptions::default()
            ),
            Err(GraphError::Empty { what: "output", .. })
        ));
        let mut s = scene(vec![]);
        s.width = 0;
        assert!(matches!(
            build(
                &s,
                Target::rgba8(10, 10),
                &source(),
                &GraphOptions::default()
            ),
            Err(GraphError::Empty { what: "canvas", .. })
        ));
    }
}
