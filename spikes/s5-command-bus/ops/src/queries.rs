//! Read-only queries: `timeline.get` and `render.frame`.

use base64::Engine as _;
use s5_bus::{
    CmdError, FrameRate, ItemId, MediaId, Query, SequenceId, Time, TrackId,
    doc::{Doc, MediaAsset, TrackKind},
    query, wire,
};

// ---------------------------------------------------------------------------------------------
// timeline.get

/// Get the timeline: tracks top to bottom with their items, plus the media they use.
#[query(
    name = "timeline.get",
    summary = "Get the timeline (tracks, items, media)",
    mcp = "core"
)]
pub struct GetTimeline {}

/// One item in a [`TrackView`].
#[wire]
pub struct ItemView {
    /// Id.
    pub id: ItemId,
    /// Source media.
    pub media: MediaId,
    /// Timeline start.
    pub start: Time,
    /// Timeline end (exclusive).
    pub end: Time,
    /// Source in-point.
    pub source_in: Time,
}

/// One track in a [`TimelineView`].
#[wire]
pub struct TrackView {
    /// Id.
    pub id: TrackId,
    /// Kind.
    pub kind: TrackKind,
    /// Name.
    pub name: String,
    /// Items sorted by start.
    pub items: Vec<ItemView>,
}

/// Result of `timeline.get`.
#[wire]
pub struct TimelineView {
    /// Project name.
    pub project: String,
    /// Sequence id.
    pub sequence: SequenceId,
    /// Sequence frame rate.
    pub frame_rate: FrameRate,
    /// End of the last item.
    pub duration: Time,
    /// Tracks, top to bottom.
    pub tracks: Vec<TrackView>,
    /// Imported media.
    pub media: Vec<MediaAsset>,
}

impl Query for GetTimeline {
    type Output = TimelineView;

    fn run(&self, doc: &Doc) -> Result<TimelineView, CmdError> {
        let tracks = doc
            .sequence
            .tracks
            .iter()
            .filter_map(|id| doc.tracks.get(id))
            .map(|t| TrackView {
                id: t.id.clone(),
                kind: t.kind,
                name: t.name.clone(),
                items: doc
                    .track_items(&t.id)
                    .into_iter()
                    .map(|i| ItemView {
                        id: i.id.clone(),
                        media: i.media.clone(),
                        start: i.start,
                        end: i.end(),
                        source_in: i.source_in,
                    })
                    .collect(),
            })
            .collect();
        Ok(TimelineView {
            project: doc.meta.name.clone(),
            sequence: doc.sequence.id.clone(),
            frame_rate: doc.sequence.frame_rate,
            duration: doc.duration(),
            tracks,
            media: doc.media.values().cloned().collect(),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// render.frame

/// Render the sequence frame at a time as a PNG (spike stub: the top-most visible video clip is a
/// flat colour derived from its media id, with a source-position bar along the bottom).
#[query(
    name = "render.frame",
    summary = "Render the frame at a time as an image",
    mcp = "core"
)]
pub struct RenderFrame {
    /// Timeline time.
    pub at: Time,
    /// Output width in pixels, 16..=768 (default 160); height follows the sequence aspect.
    pub width: Option<u32>,
}

/// Result of `render.frame`. MCP (0H.02) turns `data` into an image content block.
#[wire]
pub struct RenderedFrame {
    /// Rendered time.
    pub at: Time,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// MIME type of `data`.
    pub mime: String,
    /// Base64-encoded image bytes.
    pub data: String,
    /// Items under the playhead, top track first.
    pub visible: Vec<ItemId>,
}

fn color_of(media: &MediaId) -> [u8; 3] {
    // FNV-1a: deterministic, so golden comparisons are possible.
    let mut h: u32 = 0x811c_9dc5;
    for b in media.as_str().bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    let [r, g, b, _] = h.to_le_bytes();
    [r | 0x40, g | 0x40, b | 0x40]
}

impl Query for RenderFrame {
    type Output = RenderedFrame;

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn run(&self, doc: &Doc) -> Result<RenderedFrame, CmdError> {
        if self.at < Time::ZERO {
            return Err(CmdError::invalid("/at", "must be >= 0"));
        }
        let width = self.width.unwrap_or(160);
        if !(16..=768).contains(&width) {
            return Err(CmdError::invalid("/width", "must be within 16..=768"));
        }
        let seq = &doc.sequence;
        let height =
            (u64::from(width) * u64::from(seq.height) / u64::from(seq.width.max(1))).max(1) as u32;

        let mut visible = Vec::new();
        let mut top = None;
        for track in seq.tracks.iter().filter_map(|t| doc.tracks.get(t)) {
            if let Some(item) = doc
                .track_items(&track.id)
                .into_iter()
                .find(|i| i.start <= self.at && self.at < i.end())
            {
                visible.push(item.id.clone());
                if track.kind == TrackKind::Video && top.is_none() {
                    top = Some(item);
                }
            }
        }

        let (w, h) = (width as usize, height as usize);
        let mut pixels = vec![16_u8; w * h * 3];
        if let Some(item) = top {
            let color = color_of(&item.media);
            for px in pixels.as_chunks_mut::<3>().0 {
                *px = color;
            }
            let media_duration = doc
                .media
                .get(&item.media)
                .map_or(item.duration, |m| m.duration);
            let pos = (item.source_in + (self.at - item.start)).flicks();
            let filled =
                (i128::from(pos) * w as i128 / i128::from(media_duration.flicks().max(1))) as usize;
            for y in h.saturating_sub(2)..h {
                for x in 0..filled.min(w) {
                    let o = (y * w + x) * 3;
                    pixels[o..o + 3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }

        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let fail = |e: png::EncodingError| CmdError::Unsupported {
                msg: format!("png encoding failed: {e}"),
            };
            let mut writer = encoder.write_header().map_err(fail)?;
            writer.write_image_data(&pixels).map_err(fail)?;
        }
        Ok(RenderedFrame {
            at: self.at,
            width,
            height,
            mime: "image/png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(&png_bytes),
            visible,
        })
    }
}
