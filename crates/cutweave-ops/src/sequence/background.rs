// SPDX-License-Identifier: GPL-3.0-or-later
//! `sequence.set_background` (WP-2B.02): what a sequence draws behind its tracks. The plain
//! color is the format's `background`; a blurred copy of the picture, a still image or nothing
//! (transparent) is its `background_fill`, drawn over that color. A background has no keyframes.

use cutweave_commands::{CmdError, Command, QueryCtx, Tx, command, wire};
use cutweave_model::{
    BackgroundFill, BackgroundFit, BlurSource, Color, EntityKind, MAX_BACKGROUND_BLUR_RADIUS,
    MediaId, MediaKind, Project, SequenceFormat, SequenceId,
};

/// The blur radius a first `kind: blur` gets: 5% of the canvas height.
pub const DEFAULT_BLUR_RADIUS: f64 = 0.05;
/// The dim a first `kind: blur` gets: a quarter of the light taken away.
pub const DEFAULT_BLUR_DIM: f64 = 0.25;

/// The kinds of background.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    /// The plain color.
    Color,
    /// A blurred copy of the picture, covering the canvas.
    Blur,
    /// A still image of the project.
    Image,
    /// Nothing: a transparent canvas, for exports that keep alpha.
    Transparent,
}

/// What `sequence.set_background` returns.
#[wire]
pub struct SequenceSetBackgroundOutput {
    /// The sequence.
    pub sequence: SequenceId,
    /// Its format now.
    pub format: SequenceFormat,
}

/// Sets what a sequence draws behind its tracks. `color`: the plain color. `blur`: the picture
/// below (or the main track's) scaled to cover the canvas and blurred; `radius` is a fraction of
/// the canvas height, so preview and export match. `image`: a still of the project, sized by
/// `fit`. `transparent`: nothing, for exports that keep alpha. A field left out keeps its value
/// when the kind stays the same, else takes its default; a field of another kind is refused.
#[command(
    name = "sequence.set_background",
    summary = "Fill a sequence's background with a color, a blur of the picture, an image or nothing",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct SequenceSetBackground {
    /// The sequence.
    pub sequence: SequenceId,
    /// The kind of background.
    pub kind: BackgroundKind,
    /// `color`, `blur`, `image`: the canvas color, `#rrggbb` (under the blur or image too).
    pub color: Option<Color>,
    /// `blur`: which picture, `below` (the lowest one showing; default) or `main_track`.
    pub source: Option<BlurSource>,
    /// `blur`: the blur's size as a fraction of the canvas height, `0..=1` (default 0.05).
    pub radius: Option<f64>,
    /// `blur`: how much of the light is taken away, `0..=1` (default 0.25).
    pub dim: Option<f64>,
    /// `image`: the still image asset.
    pub media: Option<MediaId>,
    /// `image`: `cover` (default), `contain` or `stretch`.
    pub fit: Option<BackgroundFit>,
}

impl SequenceSetBackground {
    /// `Invalid` for a field that does not belong to the kind.
    fn check_fields(&self) -> Result<(), CmdError> {
        use BackgroundKind::{Blur, Color, Image};
        let given = [
            ("color", self.color.is_some(), &[Color, Blur, Image][..]),
            ("source", self.source.is_some(), &[Blur][..]),
            ("radius", self.radius.is_some(), &[Blur][..]),
            ("dim", self.dim.is_some(), &[Blur][..]),
            ("media", self.media.is_some(), &[Image][..]),
            ("fit", self.fit.is_some(), &[Image][..]),
        ];
        for (field, present, kinds) in given {
            if present && !kinds.contains(&self.kind) {
                return Err(CmdError::invalid(
                    format!("/{field}"),
                    format!("`{field}` does not apply to this kind of background"),
                ));
            }
        }
        Ok(())
    }

    /// The new format, checked against `doc`.
    fn plan(&self, doc: &Project) -> Result<SequenceFormat, CmdError> {
        let sequence = doc
            .sequences
            .get(&self.sequence)
            .ok_or_else(|| CmdError::not_found(EntityKind::Sequence, self.sequence))?;
        self.check_fields()?;
        let mut format = sequence.format.clone();
        let old = format.background_fill.take();
        format.background = self.color.unwrap_or(format.background);
        format.background_fill = match self.kind {
            BackgroundKind::Color => None,
            BackgroundKind::Transparent => Some(BackgroundFill::Transparent),
            BackgroundKind::Blur => Some(self.blur(old.as_ref())?),
            BackgroundKind::Image => Some(self.image(doc, old.as_ref())?),
        };
        Ok(format)
    }

    fn blur(&self, old: Option<&BackgroundFill>) -> Result<BackgroundFill, CmdError> {
        let (source, radius, dim) = match old {
            Some(BackgroundFill::Blur {
                source,
                radius,
                dim,
            }) => (*source, *radius, *dim),
            _ => (BlurSource::default(), DEFAULT_BLUR_RADIUS, DEFAULT_BLUR_DIM),
        };
        let radius = self.radius.unwrap_or(radius);
        let dim = self.dim.unwrap_or(dim);
        if !(0.0..=MAX_BACKGROUND_BLUR_RADIUS).contains(&radius) {
            return Err(CmdError::invalid(
                "/radius",
                "the blur radius is a fraction of the canvas height, 0 to 1",
            ));
        }
        if !(0.0..=1.0).contains(&dim) {
            return Err(CmdError::invalid("/dim", "the dim must be between 0 and 1"));
        }
        Ok(BackgroundFill::Blur {
            source: self.source.unwrap_or(source),
            radius,
            dim,
        })
    }

    fn image(
        &self,
        doc: &Project,
        old: Option<&BackgroundFill>,
    ) -> Result<BackgroundFill, CmdError> {
        let (old_media, old_fit) = match old {
            Some(BackgroundFill::Image { media, fit }) => (Some(*media), *fit),
            _ => (None, BackgroundFit::default()),
        };
        let media = self.media.or(old_media).ok_or_else(|| {
            CmdError::invalid("/media", "an image background needs `media`: a still image")
        })?;
        // The image already set may be gone (an older build can remove it): only a newly
        // named one has to exist.
        if self.media.is_some() {
            let asset = doc
                .media
                .get(&media)
                .ok_or_else(|| CmdError::not_found(EntityKind::Media, media))?;
            if asset.kind != MediaKind::Image {
                return Err(CmdError::invalid(
                    "/media",
                    "the background image must be a still image",
                ));
            }
        }
        Ok(BackgroundFill::Image {
            media,
            fit: self.fit.unwrap_or(old_fit),
        })
    }
}

impl Command for SequenceSetBackground {
    type Output = SequenceSetBackgroundOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        self.plan(ctx.doc()).map(|_| ())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<SequenceSetBackgroundOutput, CmdError> {
        let format = self.plan(tx.doc())?;
        let new = format.clone();
        tx.update_sequence(self.sequence, |s| {
            s.format = new;
            Ok(())
        })?;
        Ok(SequenceSetBackgroundOutput {
            sequence: self.sequence,
            format,
        })
    }
}
