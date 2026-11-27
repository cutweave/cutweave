// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent-vision queries (arch §14 "seeing without screenshots"): `render.frame` and
//! `render.contact_sheet`, registered with [`register`] and exposed over MCP as the core tools
//! `render_frame` / `render_contact_sheet` (`readOnlyHint`, `idempotentHint`).
//!
//! # Results
//!
//! Both return the encoded image as `mime` + base64 `data` (the S5 convention: the MCP layer
//! moves `data` into an image content block and keeps the rest as structured content), its
//! size, and metadata: the sequence, the time (flicks), frame index and timecode, the document
//! `rev`, and what is on screen ([`crate::inspect`]: visible items top first with their image
//! rectangles, hidden items with a reason). Times outside the content render the background
//! and say so in `notes`; they are never an error. The same `(rev, params)` always gives
//! byte-identical results on the same adapter (determinism, arch §8).
//!
//! # Rendering backend
//!
//! A [`Query`] only receives the document snapshot ([`QueryCtx`]), but rendering needs the
//! GPU compositor and the frame source, which belong to the engine. The engine therefore makes
//! its [`FrameBackend`] current on the dispatching thread around query calls:
//!
//! ```ignore
//! let _scope = cutweave_render::query::enter(Arc::clone(&engine.frame_backend));
//! registry.query_json("render.frame", params, &ctx)
//! ```
//!
//! so every path that runs queries through the registry (RPC, MCP core tools,
//! `cutweave_query`, the CLI) renders the same way. Outside such a scope the queries fail with
//! `Unsupported`. Rust callers can skip the registry and call [`render_frame`] /
//! [`render_contact_sheet`] with a backend directly. The GPU backend is
//! `gpu::GpuFrameBackend`; [`ReferenceBackend`] runs the f64 CPU reference renderer (tests,
//! headless builds without a GPU).
//!
//! Agent frames always use original media and are rendered directly at the requested size
//! (the compositor scales the canvas and samples mips), not rendered full-size and resized.
//! Contact-sheet cells accept proxies (preview quality).

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};

use cutweave_commands::{CmdError, Query, QueryCtx, Registry, RegistryError, Rev, query, wire};
use cutweave_model::{
    EntityKind, FrameRate, ItemId, Project, Rounding, Sequence, SequenceId, Time, TimeInput,
    Timecode,
};

use crate::encode::{
    DEFAULT_JPEG_QUALITY, DEFAULT_MAX_SIZE, ImageFormat, base64, encode, fit_within,
};
use crate::eval::evaluate;
use crate::frame::FrameSource;
use crate::graph::{GraphOptions, RenderGraph, Target, build};
use crate::image::Image;
use crate::inspect::{HiddenItem, PixelRect, VisibleItem, frame_items};
use crate::quality::{Quality, Scale};
use crate::reference;
use crate::scene::SceneDescription;
use crate::sheet::{Sampling, SheetError, SheetLayout, sample_times};

/// Smallest accepted `max_size`.
pub const MIN_MAX_SIZE: u32 = 16;
/// Largest accepted `max_size` of `render.frame`.
pub const MAX_FRAME_SIZE: u32 = 4096;
/// Smallest accepted `max_size` of `render.contact_sheet`.
pub const MIN_SHEET_SIZE: u32 = 64;
/// Largest accepted `max_size` of `render.contact_sheet`.
pub const MAX_SHEET_SIZE: u32 = 2048;
/// Items listed in `items_visible` when no `limit` is given.
pub const DEFAULT_ITEM_LIMIT: u32 = 64;
/// Cells of a contact sheet when neither `count` nor `interval` is given.
pub const DEFAULT_SHEET_COUNT: u32 = 9;

/// Renders the sequence frame at a time as an image, with what is on screen.
#[query(
    name = "render.frame",
    summary = "Render the frame at a time as an image (JPEG, ≤768 px) with the items on screen",
    mcp = "core"
)]
pub struct RenderFrame {
    /// Sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// Time: flicks, "2.5s", "f:60" or timecode "00:00:02:12".
    pub t: TimeInput,
    /// Longest image side in px (16-4096, default 768); never above the sequence size.
    pub max_size: Option<u32>,
    /// "jpeg" (default) or "png".
    #[serde(alias = "fmt")]
    pub format: Option<ImageFormat>,
    /// JPEG quality 1-100 (default 80).
    pub quality: Option<u8>,
    /// Most items to list per kind (default 64).
    pub limit: Option<u32>,
}

/// A rendered frame.
#[wire]
pub struct FrameImage {
    /// MIME type of `data`.
    pub mime: String,
    /// The image, base64 (returned over MCP as image content).
    pub data: String,
    /// Encoded size in bytes.
    pub bytes: u32,
    /// Image width in px.
    pub width: u32,
    /// Image height in px.
    pub height: u32,
    /// The sequence.
    pub sequence: SequenceId,
    /// The rendered time (flicks).
    pub time: Time,
    /// Frame index at the sequence rate (none before zero).
    pub frame: Option<u32>,
    /// Timecode of that frame (none before zero).
    pub timecode: Option<String>,
    /// Document revision rendered.
    pub rev: Option<Rev>,
    /// Items contributing pixels, top first, with their image rectangles.
    pub items_visible: Vec<VisibleItem>,
    /// Active items contributing no pixels, and why.
    pub items_hidden: Vec<HiddenItem>,
    /// Remarks (background only, missing media, truncation).
    pub notes: Vec<String>,
}

/// Renders evenly spaced frames of a range as one labelled grid image.
#[query(
    name = "render.contact_sheet",
    summary = "Render frames across a time range as one timecode-labelled grid image",
    mcp = "core"
)]
pub struct RenderContactSheet {
    /// Sequence; optional when the project has one.
    pub sequence: Option<SequenceId>,
    /// Range start (default 0).
    #[serde(alias = "from")]
    pub start: Option<TimeInput>,
    /// Range end, exclusive (default: sequence end).
    #[serde(alias = "to")]
    pub end: Option<TimeInput>,
    /// Frames, evenly spaced (default 9, max 64).
    pub count: Option<u32>,
    /// Or one frame every interval, e.g. "2s".
    #[serde(alias = "every")]
    pub interval: Option<TimeInput>,
    /// Columns (default ceil(sqrt(n))).
    #[serde(alias = "columns")]
    pub cols: Option<u32>,
    /// Longest sheet side in px (64-2048, default 768).
    pub max_size: Option<u32>,
    /// Burn timecodes under cells (default true).
    pub labels: Option<bool>,
    /// "jpeg" (default) or "png".
    #[serde(alias = "fmt")]
    pub format: Option<ImageFormat>,
}

/// One cell of a contact sheet.
#[wire]
pub struct SheetFrame {
    /// The frame time (flicks).
    pub time: Time,
    /// Frame index at the sequence rate.
    pub frame: Option<u32>,
    /// Timecode (the cell label).
    pub timecode: Option<String>,
    /// The cell's picture on the sheet.
    pub cell: PixelRect,
    /// Items contributing pixels, top first.
    pub items_visible: Vec<ItemId>,
}

/// A rendered contact sheet.
#[wire]
pub struct ContactSheet {
    /// MIME type of `data`.
    pub mime: String,
    /// The image, base64 (returned over MCP as image content).
    pub data: String,
    /// Encoded size in bytes.
    pub bytes: u32,
    /// Sheet width in px.
    pub width: u32,
    /// Sheet height in px.
    pub height: u32,
    /// The sequence.
    pub sequence: SequenceId,
    /// Document revision rendered.
    pub rev: Option<Rev>,
    /// Grid columns.
    pub cols: u32,
    /// Grid rows.
    pub rows: u32,
    /// The cells, row by row.
    pub frames: Vec<SheetFrame>,
    /// Remarks (missing media).
    pub notes: Vec<String>,
}

/// Registers `render.frame` and `render.contact_sheet`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<RenderFrame>()?
        .register::<RenderContactSheet>()?;
    Ok(())
}

/// What renders frames for the queries: the GPU compositor in the engine, or the CPU reference.
pub trait FrameBackend {
    /// Where layer frames come from (decoder pool / frame cache).
    fn frame_source(&self) -> &dyn FrameSource;

    /// Settings that change pixels (tone map).
    fn graph_options(&self) -> GraphOptions {
        GraphOptions::default()
    }

    /// Renders `graph` (compiled from `scene` for an RGBA8 target by the caller) to
    /// straight-alpha sRGB RGBA8 of the target size.
    fn rasterize(&mut self, scene: &SceneDescription, graph: &RenderGraph)
    -> Result<Image, String>;
}

/// A backend shared between the engine and the threads running queries.
pub type SharedBackend = Arc<Mutex<dyn FrameBackend + Send>>;

thread_local! {
    static SCOPE: RefCell<Vec<SharedBackend>> = const { RefCell::new(Vec::new()) };
}

/// Keeps a backend current on this thread (see the module docs); dropping it restores the
/// previous one. Not `Send`: it must be dropped on the thread that created it.
#[derive(Debug)]
#[must_use = "the backend is current only while the scope guard lives"]
pub struct BackendScope {
    _thread_bound: PhantomData<*const ()>,
}

impl Drop for BackendScope {
    fn drop(&mut self) {
        SCOPE.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Makes `backend` render the `render.*` queries run on this thread until the returned guard
/// is dropped. Scopes nest.
pub fn enter(backend: SharedBackend) -> BackendScope {
    SCOPE.with(|s| s.borrow_mut().push(backend));
    BackendScope {
        _thread_bound: PhantomData,
    }
}

/// Runs `f` with the current backend, or fails with `Unsupported` when there is none.
fn with_current<R>(
    name: &str,
    f: impl FnOnce(&mut dyn FrameBackend) -> Result<R, CmdError>,
) -> Result<R, CmdError> {
    let backend = SCOPE.with(|s| s.borrow().last().cloned()).ok_or_else(|| {
        CmdError::unsupported(format!(
            "`{name}` needs the engine's renderer; call it through the engine (MCP, RPC or CLI)"
        ))
    })?;
    let mut guard = backend.lock().unwrap_or_else(PoisonError::into_inner);
    f(&mut *guard)
}

impl Query for RenderFrame {
    type Output = FrameImage;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<FrameImage, CmdError> {
        with_current("render.frame", |b| render_frame(b, ctx, self))
    }
}

impl Query for RenderContactSheet {
    type Output = ContactSheet;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<ContactSheet, CmdError> {
        with_current("render.contact_sheet", |b| {
            render_contact_sheet(b, ctx, self)
        })
    }
}

/// The CPU reference renderer as a backend (exact `f64` semantics, slow): tests and builds
/// without a GPU.
pub struct ReferenceBackend {
    source: Arc<dyn FrameSource + Send + Sync>,
    options: GraphOptions,
}

impl ReferenceBackend {
    /// A backend over `source`.
    #[must_use]
    pub fn new(source: Arc<dyn FrameSource + Send + Sync>) -> ReferenceBackend {
        ReferenceBackend {
            source,
            options: GraphOptions::default(),
        }
    }

    /// Shares it for [`enter`].
    #[must_use]
    pub fn shared(self) -> SharedBackend {
        Arc::new(Mutex::new(self))
    }
}

impl FrameBackend for ReferenceBackend {
    fn frame_source(&self) -> &dyn FrameSource {
        &*self.source
    }

    fn graph_options(&self) -> GraphOptions {
        self.options
    }

    fn rasterize(
        &mut self,
        scene: &SceneDescription,
        graph: &RenderGraph,
    ) -> Result<Image, String> {
        reference::render(scene, graph.target, &*self.source, &self.options)
            .map_err(|e| e.to_string())
    }
}

/// The sequence named by `id`, or the project's only sequence.
fn resolve_sequence(
    doc: &Project,
    id: Option<SequenceId>,
) -> Result<(SequenceId, &Sequence), CmdError> {
    if let Some(id) = id {
        return doc
            .sequences
            .get(&id)
            .map(|s| (id, s))
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, id));
    }
    let mut all = doc.sequences.iter();
    match (all.next(), all.next()) {
        (Some((id, s)), None) => Ok((*id, s)),
        (None, _) => Err(CmdError::invalid(
            "/sequence",
            "the project has no sequence",
        )),
        (Some(_), Some(_)) => Err(CmdError::invalid(
            "/sequence",
            format!(
                "the project has {} sequences; name one",
                doc.sequences.len()
            ),
        )),
    }
}

fn resolve_time(input: TimeInput, rate: FrameRate, path: &str) -> Result<Time, CmdError> {
    input
        .resolve(Some(rate))
        .map_err(|e| CmdError::invalid(path, e.to_string()))
}

fn in_range(
    value: Option<u32>,
    default: u32,
    lo: u32,
    hi: u32,
    path: &str,
) -> Result<u32, CmdError> {
    let v = value.unwrap_or(default);
    if (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(CmdError::invalid(
            path,
            format!("must be within {lo}..={hi}"),
        ))
    }
}

/// Frame index and timecode of `t` (none before zero).
fn frame_label(t: Time, rate: FrameRate) -> (Option<u32>, Option<String>) {
    if t < Time::ZERO {
        return (None, None);
    }
    let frame = rate.time_to_frame(t, Rounding::Floor);
    let drop_frame = rate.drop_frames_per_minute().is_some();
    let tc = Timecode::from_frame(frame, rate, drop_frame)
        .ok()
        .map(|tc| tc.to_string());
    (u32::try_from(frame).ok(), tc)
}

fn encode_image(
    image: &Image,
    format: ImageFormat,
    quality: u8,
) -> Result<(String, u32), CmdError> {
    let mut bytes = Vec::new();
    encode(image, format, quality, &mut bytes)
        .map_err(|e| CmdError::internal(format!("image encoding failed: {e}")))?;
    Ok((
        base64(&bytes),
        u32::try_from(bytes.len()).unwrap_or(u32::MAX),
    ))
}

/// One rendered picture and what it shows.
struct Shot {
    image: Image,
    visible: Vec<VisibleItem>,
    hidden: Vec<HiddenItem>,
    background_only: bool,
    missing: Vec<String>,
}

/// Evaluates `seq` at `t`, renders it at `size` with `quality` and inspects it.
fn shoot(
    backend: &mut dyn FrameBackend,
    doc: &Project,
    seq: SequenceId,
    t: Time,
    size: (u32, u32),
    quality: Quality,
) -> Result<Shot, CmdError> {
    let mut scene = evaluate(doc, seq, t).map_err(|e| CmdError::internal(e.to_string()))?;
    quality.apply(&mut scene);
    let graph = build(
        &scene,
        Target::rgba8(size.0, size.1),
        backend.frame_source(),
        &backend.graph_options(),
    )
    .map_err(|e| CmdError::internal(format!("cannot compile the frame: {e}")))?;
    let (visible, hidden) = frame_items(&scene, &graph);
    let image = backend
        .rasterize(&scene, &graph)
        .map_err(|e| CmdError::internal(format!("rendering failed: {e}")))?;
    let missing = graph
        .missing
        .iter()
        .map(|(item, e)| format!("item {item}: {e}"))
        .collect();
    Ok(Shot {
        image,
        visible,
        hidden,
        background_only: scene.layers.is_empty(),
        missing,
    })
}

/// The frame of `sequence` at `t` at the full sequence size, with the pixels `render.frame`
/// gives at that time (full quality, original media): what `media.import_frame` saves as a
/// picture (ADR-0035). Unlike `render.frame` it has no size limit below the sequence's own.
///
/// # Errors
/// `NotFound` for a sequence `doc` does not have; `Internal` when the frame cannot be
/// evaluated, compiled or rendered.
pub fn render_full_frame(
    backend: &mut dyn FrameBackend,
    doc: &Project,
    sequence: SequenceId,
    t: Time,
) -> Result<Image, CmdError> {
    let (id, seq) = resolve_sequence(doc, Some(sequence))?;
    let size = (seq.format.width, seq.format.height);
    Ok(shoot(backend, doc, id, t, size, Quality::FULL)?.image)
}

/// `render.frame` with an explicit backend.
pub fn render_frame(
    backend: &mut dyn FrameBackend,
    ctx: &QueryCtx<'_>,
    params: &RenderFrame,
) -> Result<FrameImage, CmdError> {
    let doc = ctx.doc();
    let (seq_id, seq) = resolve_sequence(doc, params.sequence)?;
    let rate = seq.format.frame_rate;
    let t = resolve_time(params.t, rate, "/t")?;
    let max_size = in_range(
        params.max_size,
        DEFAULT_MAX_SIZE,
        MIN_MAX_SIZE,
        MAX_FRAME_SIZE,
        "/max_size",
    )?;
    let quality = in_range(
        params.quality.map(u32::from),
        u32::from(DEFAULT_JPEG_QUALITY),
        1,
        100,
        "/quality",
    )?;
    let limit = params.limit.unwrap_or(DEFAULT_ITEM_LIMIT) as usize;
    let format = params.format.unwrap_or_default();

    let size = fit_within((seq.format.width, seq.format.height), max_size);
    let mut shot = shoot(backend, doc, seq_id, t, size, Quality::FULL)?;
    let mut notes = Vec::new();
    if shot.background_only {
        notes.push("nothing is visible at this time: the image is the sequence background".into());
    }
    notes.append(&mut shot.missing);
    if shot.visible.len() > limit {
        notes.push(format!(
            "items_visible truncated to {limit} of {}",
            shot.visible.len()
        ));
        shot.visible.truncate(limit);
    }
    if shot.hidden.len() > limit {
        notes.push(format!(
            "items_hidden truncated to {limit} of {}",
            shot.hidden.len()
        ));
        shot.hidden.truncate(limit);
    }
    #[allow(clippy::cast_possible_truncation)]
    let (data, bytes) = encode_image(&shot.image, format, quality as u8)?;
    let (frame, timecode) = frame_label(t, rate);
    Ok(FrameImage {
        mime: format.mime().into(),
        data,
        bytes,
        width: shot.image.width,
        height: shot.image.height,
        sequence: seq_id,
        time: t,
        frame,
        timecode,
        rev: ctx.rev(),
        items_visible: shot.visible,
        items_hidden: shot.hidden,
        notes,
    })
}

fn sheet_error(e: &SheetError, sampling_path: &str) -> CmdError {
    let path = match e {
        SheetError::EmptyRange => "/end",
        SheetError::TooSmall { .. } => "/max_size",
        SheetError::NothingToSample | SheetError::TooMany(_) => sampling_path,
    };
    CmdError::invalid(path, e.to_string())
}

/// A contact sheet before encoding.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposedSheet {
    /// The sheet pixels.
    pub image: Image,
    /// The sequence.
    pub sequence: SequenceId,
    /// Grid columns.
    pub cols: u32,
    /// Grid rows.
    pub rows: u32,
    /// The cells, row by row.
    pub frames: Vec<SheetFrame>,
    /// Remarks (missing media).
    pub notes: Vec<String>,
}

/// Samples, renders and lays out a contact sheet (everything `render.contact_sheet` does but
/// encoding), with an explicit backend.
pub fn compose_contact_sheet(
    backend: &mut dyn FrameBackend,
    ctx: &QueryCtx<'_>,
    params: &RenderContactSheet,
) -> Result<ComposedSheet, CmdError> {
    let doc = ctx.doc();
    let (seq_id, seq) = resolve_sequence(doc, params.sequence)?;
    let rate = seq.format.frame_rate;
    let start = match params.start {
        Some(s) => resolve_time(s, rate, "/start")?,
        None => Time::ZERO,
    };
    let end = if let Some(e) = params.end {
        resolve_time(e, rate, "/end")?
    } else if seq.duration().is_zero() {
        return Err(CmdError::invalid(
            "/end",
            "the sequence is empty; give `end` to sample a range",
        ));
    } else {
        Time::from(seq.duration())
    };
    let (sampling, sampling_path) = match (params.count, params.interval) {
        (Some(_), Some(_)) => {
            return Err(CmdError::invalid(
                "",
                "give either `count` or `interval`, not both",
            ));
        }
        (Some(n), None) => (Sampling::Count(n), "/count"),
        (None, Some(i)) => {
            let i = resolve_time(i, rate, "/interval")?;
            let d = cutweave_model::Duration::try_from(i)
                .map_err(|_| CmdError::invalid("/interval", "must be positive"))?;
            (Sampling::Interval(d), "/interval")
        }
        (None, None) => (Sampling::Count(DEFAULT_SHEET_COUNT), "/count"),
    };
    let max_size = in_range(
        params.max_size,
        DEFAULT_MAX_SIZE,
        MIN_SHEET_SIZE,
        MAX_SHEET_SIZE,
        "/max_size",
    )?;
    let times =
        sample_times(start, end, sampling, rate).map_err(|e| sheet_error(&e, sampling_path))?;
    let count = u32::try_from(times.len()).unwrap_or(u32::MAX);
    let layout = SheetLayout::new(
        count,
        params.cols,
        (seq.format.width, seq.format.height),
        max_size,
        params.labels.unwrap_or(true),
    )
    .map_err(|e| sheet_error(&e, sampling_path))?;

    let mut image = layout.canvas();
    let mut frames = Vec::with_capacity(times.len());
    let mut notes = Vec::new();
    // Cells are small: proxies are fine (preview quality); the cell size sets the resolution.
    let cell_quality = Quality::new(Scale::Full, true);
    for (index, &t) in (0u32..).zip(&times) {
        let shot = shoot(
            backend,
            doc,
            seq_id,
            t,
            (layout.cell_width, layout.cell_height),
            cell_quality,
        )?;
        let (frame, timecode) = frame_label(t, rate);
        layout.place(
            &mut image,
            index,
            &shot.image,
            timecode.as_deref().unwrap_or(""),
        );
        for m in shot.missing {
            if !notes.contains(&m) {
                notes.push(m);
            }
        }
        frames.push(SheetFrame {
            time: t,
            frame,
            timecode,
            cell: layout.cell(index),
            items_visible: shot.visible.iter().map(|v| v.item).collect(),
        });
    }
    Ok(ComposedSheet {
        image,
        sequence: seq_id,
        cols: layout.cols,
        rows: layout.rows,
        frames,
        notes,
    })
}

/// `render.contact_sheet` with an explicit backend.
pub fn render_contact_sheet(
    backend: &mut dyn FrameBackend,
    ctx: &QueryCtx<'_>,
    params: &RenderContactSheet,
) -> Result<ContactSheet, CmdError> {
    let sheet = compose_contact_sheet(backend, ctx, params)?;
    let format = params.format.unwrap_or_default();
    let (data, bytes) = encode_image(&sheet.image, format, DEFAULT_JPEG_QUALITY)?;
    Ok(ContactSheet {
        mime: format.mime().into(),
        data,
        bytes,
        width: sheet.image.width,
        height: sheet.image.height,
        sequence: sheet.sequence,
        rev: ctx.rev(),
        cols: sheet.cols,
        rows: sheet.rows,
        frames: sheet.frames,
        notes: sheet.notes,
    })
}
