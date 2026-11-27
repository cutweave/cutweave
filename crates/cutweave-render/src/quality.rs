// SPDX-License-Identifier: GPL-3.0-or-later
//! The preview quality ladder (arch §8): full, ½ or ¼ resolution, plus proxy or original media,
//! and the [`AutoQuality`] policy the transport (WP-1H.01) drives with measured frame times.
//!
//! * [`Quality`] is one rung: an output [`Scale`] applied to the size the frame is rendered at
//!   (the preview canvas, arch §10, or the sequence size) and whether layers may be served from
//!   proxies ([`FrameRequest::proxy`](crate::FrameRequest::proxy)). The scale is applied at
//!   output size; decoding at reduced size is future work. Layout never depends on the rung:
//!   geometry uses the probed source size, so every rung shows the same picture, only sharper
//!   or softer.
//!   The preview goes further ([`Quality::apply_for_output`], WP-2F.02): `use_proxies` is the
//!   preview's toggle (`view.set_preview_quality`), set by the transport on every rung, and each
//!   layer is told how large its picture is drawn, so a proxy is served only when it holds
//!   those pixels: a full-resolution preview that needs more than the proxy has shows the
//!   original.
//! * [`Ladder`] orders the rungs best first. [`Ladder::standard`] is full → ½ → ¼ on originals,
//!   or, when proxies exist, original → proxy at full → ½ → ¼ on proxies.
//! * [`AutoQuality`] adapts during playback and refines when paused: it drops one rung after
//!   [`AutoQualityConfig::late_frames`] consecutive frames slower than the frame interval
//!   (default 5), climbs one rung after [`AutoQualityConfig::fast_frames`] consecutive frames
//!   that took at most [`AutoQualityConfig::fast_fraction`] of it, and backs off (doubles the
//!   fast streak needed, up to 16×) when a climb is immediately undone, so it does not
//!   oscillate. Paused is always [`Quality::FULL`]; scrubbing uses the bottom rung first (the
//!   scheduler refines to full when idle, arch §10 step 5). Playback resumes at the rung the
//!   last playback settled on.
//!
//! Export is always [`Quality::EXPORT`] (full, originals) and agent frames (`render.frame`) use
//! originals at the requested size. Everything here is pure: frame times are measured by the
//! caller, so the policy holds no clock (determinism, arch §8) and is unit-tested with
//! simulated times.

use core::time::Duration;

use cutweave_model::FrameRate;
use serde::{Deserialize, Serialize};

use crate::scene::{Layer, SceneDescription};

/// Output resolution relative to the base size.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    /// The base size.
    #[default]
    Full,
    /// Half the width and height (a quarter of the pixels).
    Half,
    /// A quarter of the width and height (a sixteenth of the pixels).
    Quarter,
}

impl Scale {
    /// The divisor of each dimension: 1, 2 or 4.
    #[must_use]
    pub const fn divisor(self) -> u32 {
        match self {
            Scale::Full => 1,
            Scale::Half => 2,
            Scale::Quarter => 4,
        }
    }

    /// `(width, height)` divided by [`Scale::divisor`], rounded up, at least 1×1.
    #[must_use]
    pub const fn apply(self, (width, height): (u32, u32)) -> (u32, u32) {
        let d = self.divisor();
        let w = width.div_ceil(d);
        let h = height.div_ceil(d);
        (if w == 0 { 1 } else { w }, if h == 0 { 1 } else { h })
    }
}

/// One rung of the quality ladder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Quality {
    /// Output resolution relative to the base size.
    pub scale: Scale,
    /// Whether layers may be served from proxies instead of the original media.
    pub use_proxies: bool,
}

impl Quality {
    /// Full resolution from the original media.
    pub const FULL: Quality = Quality::new(Scale::Full, false);
    /// What export always renders at (arch §8): [`Quality::FULL`].
    pub const EXPORT: Quality = Quality::FULL;

    /// A rung.
    #[must_use]
    pub const fn new(scale: Scale, use_proxies: bool) -> Quality {
        Quality { scale, use_proxies }
    }

    /// Whether this is [`Quality::FULL`].
    #[must_use]
    pub const fn is_full(self) -> bool {
        matches!(self.scale, Scale::Full) && !self.use_proxies
    }

    /// The output size for a `base` size (the preview canvas or the sequence size).
    #[must_use]
    pub const fn output_size(self, base: (u32, u32)) -> (u32, u32) {
        self.scale.apply(base)
    }

    /// Marks every layer of `scene` as accepting a proxy (or not), so the
    /// [`FrameSource`](crate::FrameSource) can serve one.
    ///
    /// A blurred background reads its layer's frame, so it follows; a background image always
    /// uses the original.
    pub fn apply(self, scene: &mut SceneDescription) {
        for layer in &mut scene.layers {
            layer.request.proxy = self.use_proxies;
            layer.request.drawn_long_edge = None;
        }
    }

    /// [`apply`](Self::apply) for a frame rendered `output` pixels large (the preview): also
    /// tells each layer how large its picture is drawn
    /// ([`FrameRequest::drawn_long_edge`](crate::FrameRequest::drawn_long_edge)), so the media
    /// layer serves a proxy only when it holds those pixels (WP-2F.02's resolution rule; a
    /// full-resolution preview that needs more gets the original).
    pub fn apply_for_output(self, scene: &mut SceneDescription, output: (u32, u32)) {
        let canvas = (scene.width, scene.height);
        for layer in &mut scene.layers {
            layer.request.proxy = self.use_proxies;
            layer.request.drawn_long_edge = if self.use_proxies {
                drawn_long_edge(layer, canvas, output)
            } else {
                None
            };
        }
    }
}

/// The longest side, in output pixels, the whole upright picture of `layer` is drawn at when a
/// `canvas`-sized scene is rendered `output` pixels large: the aspect-fit size times the
/// layer's scale, in the output's pixels, rounded up; at most the original's own long side
/// (nothing needs more pixels than the file has). Rotation about the anchor and crop do not
/// change it. `None` without a probed size.
fn drawn_long_edge(layer: &Layer, canvas: (u32, u32), output: (u32, u32)) -> Option<u32> {
    let (w, h) = layer.request.source_size?;
    if w == 0 || h == 0 || canvas.0 == 0 || canvas.1 == 0 {
        return None;
    }
    // The upright picture (the display rotation swaps the sides).
    let (sw, sh) = if layer.request.rotation % 180 == 90 {
        (f64::from(h), f64::from(w))
    } else {
        (f64::from(w), f64::from(h))
    };
    let (cw, ch) = (f64::from(canvas.0), f64::from(canvas.1));
    let fit = (cw / sw).min(ch / sh);
    let drawn_w = sw * fit * layer.transform.scale.x.abs() * f64::from(output.0) / cw;
    let drawn_h = sh * fit * layer.transform.scale.y.abs() * f64::from(output.1) / ch;
    let drawn = drawn_w.max(drawn_h).ceil();
    let original = w.max(h);
    if !drawn.is_finite() || drawn >= f64::from(original) {
        return Some(original);
    }
    // In range: `0 <= drawn < original <= u32::MAX`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(drawn as u32)
}

/// The rungs the policy moves between, best first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ladder {
    rungs: Vec<Quality>,
}

impl Ladder {
    /// Full → ½ → ¼ on originals; with proxies, original → proxy at full → ½ → ¼ on proxies.
    #[must_use]
    pub fn standard(proxies_available: bool) -> Ladder {
        let rungs = if proxies_available {
            vec![
                Quality::FULL,
                Quality::new(Scale::Full, true),
                Quality::new(Scale::Half, true),
                Quality::new(Scale::Quarter, true),
            ]
        } else {
            vec![
                Quality::FULL,
                Quality::new(Scale::Half, false),
                Quality::new(Scale::Quarter, false),
            ]
        };
        Ladder { rungs }
    }

    /// A custom ladder (best first); `None` when empty.
    #[must_use]
    pub fn new(rungs: Vec<Quality>) -> Option<Ladder> {
        (!rungs.is_empty()).then_some(Ladder { rungs })
    }

    /// The rungs, best first.
    #[must_use]
    pub fn rungs(&self) -> &[Quality] {
        &self.rungs
    }

    /// The best rung.
    #[must_use]
    pub fn top(&self) -> Quality {
        self.rungs[0]
    }

    /// The cheapest rung.
    #[must_use]
    pub fn bottom(&self) -> Quality {
        self.rungs[self.rungs.len() - 1]
    }

    fn last(&self) -> usize {
        self.rungs.len() - 1
    }
}

impl Default for Ladder {
    fn default() -> Ladder {
        Ladder::standard(false)
    }
}

/// Tuning of [`AutoQuality`].
#[derive(Clone, Debug, PartialEq)]
pub struct AutoQualityConfig {
    /// The rungs.
    pub ladder: Ladder,
    /// Consecutive late frames (slower than the frame interval) that drop one rung.
    pub late_frames: u32,
    /// Consecutive fast frames that climb one rung (before back-off).
    pub fast_frames: u32,
    /// A frame is fast when it takes at most this fraction of the frame interval.
    pub fast_fraction: f64,
}

impl Default for AutoQualityConfig {
    fn default() -> AutoQualityConfig {
        AutoQualityConfig {
            ladder: Ladder::default(),
            late_frames: 5,
            fast_frames: 30,
            fast_fraction: 0.5,
        }
    }
}

/// What the transport is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    /// Stopped or paused: refine to full quality.
    #[default]
    Paused,
    /// Playing: adapt to hold the frame rate.
    Playing,
    /// Scrubbing: the cheapest rung first (the scheduler refines when idle).
    Scrubbing,
}

/// Largest back-off multiplier of the fast streak.
const MAX_BACKOFF: u32 = 16;

/// The adaptive quality policy (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct AutoQuality {
    config: AutoQualityConfig,
    interval: Duration,
    mode: PlaybackMode,
    /// The playback rung (index into the ladder), kept across pauses.
    level: usize,
    late: u32,
    fast: u32,
    /// Multiplier of `fast_frames` after climbs that were undone.
    backoff: u32,
    /// The last change was a climb not yet confirmed by a full fast streak at the new rung.
    probation: bool,
}

impl AutoQuality {
    /// The default policy for playback at `rate`.
    #[must_use]
    pub fn new(rate: FrameRate) -> AutoQuality {
        AutoQuality::with_config(rate, AutoQualityConfig::default())
    }

    /// A policy with explicit tuning.
    #[must_use]
    pub fn with_config(rate: FrameRate, config: AutoQualityConfig) -> AutoQuality {
        AutoQuality {
            config,
            interval: frame_interval(rate),
            mode: PlaybackMode::Paused,
            level: 0,
            late: 0,
            fast: 0,
            backoff: 1,
            probation: false,
        }
    }

    /// The tuning.
    #[must_use]
    pub fn config(&self) -> &AutoQualityConfig {
        &self.config
    }

    /// The frame interval frames are measured against.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Changes the target rate (the sequence rate, or the delivered rate when playing faster).
    pub fn set_frame_rate(&mut self, rate: FrameRate) {
        self.interval = frame_interval(rate);
        self.late = 0;
        self.fast = 0;
    }

    /// The current mode.
    #[must_use]
    pub fn mode(&self) -> PlaybackMode {
        self.mode
    }

    /// The playback rung index (0 = best), kept while paused.
    #[must_use]
    pub fn level(&self) -> usize {
        self.level
    }

    /// The quality to render the next frame at.
    #[must_use]
    pub fn quality(&self) -> Quality {
        match self.mode {
            PlaybackMode::Paused => Quality::FULL,
            PlaybackMode::Playing => self.config.ladder.rungs[self.level],
            PlaybackMode::Scrubbing => self.config.ladder.bottom(),
        }
    }

    /// Switches mode and returns the quality to render at. Pausing refines to full quality;
    /// playing resumes at the last playback rung. Streaks restart.
    pub fn set_mode(&mut self, mode: PlaybackMode) -> Quality {
        if mode != self.mode {
            self.mode = mode;
            self.late = 0;
            self.fast = 0;
            self.probation = false;
        }
        self.quality()
    }

    /// Back to the best rung with no history.
    pub fn reset(&mut self) {
        self.level = 0;
        self.late = 0;
        self.fast = 0;
        self.backoff = 1;
        self.probation = false;
    }

    /// Records how long the last frame took (render + readback, measured by the caller) and
    /// returns the quality for the next one. Only playback frames move the ladder.
    pub fn record_frame(&mut self, frame_time: Duration) -> Quality {
        if self.mode != PlaybackMode::Playing {
            return self.quality();
        }
        let late = frame_time > self.interval;
        let fast = self.interval.as_secs_f64() * self.config.fast_fraction;
        let fast = !late && frame_time.as_secs_f64() <= fast;
        if late {
            self.late += 1;
            self.fast = 0;
        } else {
            self.late = 0;
            self.fast = if fast { self.fast + 1 } else { 0 };
        }
        let needed_fast = self.config.fast_frames.max(1).saturating_mul(self.backoff);
        if self.late >= self.config.late_frames.max(1) {
            if self.level < self.config.ladder.last() {
                self.level += 1;
                if self.probation {
                    // The climb did not hold: wait longer before trying again.
                    self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
                }
            }
            self.probation = false;
            self.late = 0;
            self.fast = 0;
        } else if self.fast >= needed_fast {
            if self.probation {
                // A full fast streak at the new rung: the climb held.
                self.probation = false;
                self.backoff = 1;
            }
            if self.level > 0 {
                self.level -= 1;
                self.probation = true;
            }
            self.fast = 0;
        }
        self.quality()
    }
}

/// `1 / rate`, at least 1 ns.
fn frame_interval(rate: FrameRate) -> Duration {
    let nanos = u64::from(rate.den()) * 1_000_000_000 / u64::from(rate.num()).max(1);
    Duration::from_nanos(nanos.max(1))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_possible_truncation)]

    use cutweave_model::{ColorTags, FrameRate, ItemId, MediaId, SequenceId, Time, TrackId, Vec2};

    use super::*;
    use crate::scene::{BlendMode, Crop, Flip, FrameRequest, Layer, LayerTransform};

    const FPS30: FrameRate = FrameRate::FPS_30;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    fn playing() -> AutoQuality {
        let mut q = AutoQuality::new(FPS30);
        q.set_mode(PlaybackMode::Playing);
        q
    }

    fn feed(q: &mut AutoQuality, n: u32, t: Duration) -> Quality {
        let mut last = q.quality();
        for _ in 0..n {
            last = q.record_frame(t);
        }
        last
    }

    #[test]
    fn scales_divide_the_base_size_rounding_up() {
        assert_eq!(Scale::Full.apply((1920, 1080)), (1920, 1080));
        assert_eq!(Scale::Half.apply((1920, 1080)), (960, 540));
        assert_eq!(Scale::Quarter.apply((1920, 1080)), (480, 270));
        assert_eq!(Scale::Half.apply((1279, 719)), (640, 360));
        assert_eq!(Scale::Quarter.apply((1, 1)), (1, 1));
        assert_eq!(Scale::Quarter.apply((0, 3)), (1, 1));
        assert_eq!(
            Quality::new(Scale::Half, true).output_size((100, 50)),
            (50, 25)
        );
        assert!(Quality::FULL.is_full() && Quality::EXPORT == Quality::FULL);
        assert!(!Quality::new(Scale::Full, true).is_full());
    }

    #[test]
    fn standard_ladders() {
        let plain = Ladder::standard(false);
        assert_eq!(
            plain
                .rungs()
                .iter()
                .map(|q| (q.scale, q.use_proxies))
                .collect::<Vec<_>>(),
            [
                (Scale::Full, false),
                (Scale::Half, false),
                (Scale::Quarter, false)
            ]
        );
        let proxies = Ladder::standard(true);
        assert_eq!(proxies.top(), Quality::FULL);
        assert_eq!(proxies.rungs()[1], Quality::new(Scale::Full, true));
        assert_eq!(proxies.bottom(), Quality::new(Scale::Quarter, true));
        assert!(Ladder::new(Vec::new()).is_none());
    }

    #[test]
    fn five_consecutive_late_frames_drop_one_rung() {
        let mut q = playing();
        assert_eq!(q.interval(), Duration::from_nanos(33_333_333));
        // Four late frames, then an on-time one: the streak restarts.
        assert_eq!(feed(&mut q, 4, ms(40)), Quality::FULL);
        assert_eq!(q.record_frame(ms(30)), Quality::FULL);
        assert_eq!(feed(&mut q, 4, ms(40)), Quality::FULL);
        // The fifth consecutive late frame drops exactly one rung.
        assert_eq!(q.record_frame(ms(40)), Quality::new(Scale::Half, false));
        assert_eq!(q.level(), 1);
        // Five more: quarter; then the bottom holds.
        assert_eq!(feed(&mut q, 4, ms(40)).scale, Scale::Half);
        assert_eq!(q.record_frame(ms(40)).scale, Scale::Quarter);
        assert_eq!(feed(&mut q, 50, ms(200)).scale, Scale::Quarter);
        assert_eq!(q.level(), 2);
    }

    #[test]
    fn exactly_on_budget_is_not_late() {
        let mut q = playing();
        let interval = q.interval();
        assert_eq!(feed(&mut q, 20, interval), Quality::FULL);
        assert_eq!(
            feed(&mut q, 5, interval + Duration::from_nanos(1)).scale,
            Scale::Half
        );
    }

    #[test]
    fn pausing_refines_to_full_and_playback_resumes_where_it_settled() {
        let mut q = playing();
        feed(&mut q, 10, ms(50));
        assert_eq!(q.quality().scale, Scale::Quarter);
        // Paused → full, whatever the history; frames recorded while paused change nothing.
        assert_eq!(q.set_mode(PlaybackMode::Paused), Quality::FULL);
        assert_eq!(feed(&mut q, 20, ms(500)), Quality::FULL);
        assert_eq!(q.level(), 2);
        // Scrubbing starts at the bottom rung.
        assert_eq!(
            q.set_mode(PlaybackMode::Scrubbing),
            Quality::new(Scale::Quarter, false)
        );
        // Playback resumes at the settled rung.
        assert_eq!(q.set_mode(PlaybackMode::Playing).scale, Scale::Quarter);
        q.reset();
        assert_eq!(q.quality(), Quality::FULL);
    }

    #[test]
    fn fast_frames_climb_and_a_failed_climb_backs_off() {
        let mut q = playing();
        feed(&mut q, 10, ms(50));
        assert_eq!(q.level(), 2);
        // 29 fast frames are not enough; the 30th climbs one rung.
        assert_eq!(feed(&mut q, 29, ms(10)).scale, Scale::Quarter);
        assert_eq!(q.record_frame(ms(10)).scale, Scale::Half);
        // A middling frame (between half and full budget) breaks the fast streak.
        feed(&mut q, 29, ms(10));
        q.record_frame(ms(25));
        assert_eq!(feed(&mut q, 29, ms(10)).scale, Scale::Half);
        assert_eq!(q.record_frame(ms(10)).scale, Scale::Full);
        // Each climb undone right away doubles the streak the next climb needs, up to 16×.
        for multiplier in [2, 4, 8, 16, 16] {
            assert_eq!(feed(&mut q, 5, ms(40)).scale, Scale::Half);
            assert_eq!(
                feed(&mut q, 30 * multiplier - 1, ms(10)).scale,
                Scale::Half,
                "{multiplier}x"
            );
            assert_eq!(q.record_frame(ms(10)).scale, Scale::Full, "{multiplier}x");
        }
        // A climb that holds for a full streak resets the back-off.
        feed(&mut q, 30 * 16, ms(10));
        assert_eq!(feed(&mut q, 5, ms(40)).scale, Scale::Half);
        assert_eq!(feed(&mut q, 30, ms(10)).scale, Scale::Full);
    }

    #[test]
    fn the_frame_rate_sets_the_budget() {
        let mut q = AutoQuality::new(FrameRate::FPS_60);
        q.set_mode(PlaybackMode::Playing);
        // 20 ms is on time at 30 fps but late at 60.
        assert_eq!(feed(&mut q, 5, ms(20)).scale, Scale::Half);
        q.set_frame_rate(FrameRate::FPS_24);
        assert_eq!(q.interval(), Duration::from_nanos(41_666_666));
        assert_eq!(feed(&mut q, 10, ms(30)).scale, Scale::Half);
    }

    #[test]
    fn custom_ladders_and_proxy_marking() {
        let ladder = Ladder::standard(true);
        let mut q = AutoQuality::with_config(
            FPS30,
            AutoQualityConfig {
                ladder,
                ..AutoQualityConfig::default()
            },
        );
        q.set_mode(PlaybackMode::Playing);
        assert_eq!(feed(&mut q, 5, ms(40)), Quality::new(Scale::Full, true));

        let layer = Layer {
            item: ItemId::from_u128(1),
            track: TrackId::from_u128(1),
            request: FrameRequest {
                media: MediaId::from_u128(1),
                stream: Some(0),
                source_time: Time::ZERO,
                source_size: Some((64, 36)),
                rotation: 0,
                color: ColorTags::default(),
                still: false,
                proxy: false,
                drawn_long_edge: None,
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
            flip: Flip::default(),
            effects: Vec::new(),
        };
        let mut scene = SceneDescription {
            sequence: SequenceId::from_u128(1),
            time: Time::ZERO,
            width: 64,
            height: 36,
            frame_rate: FPS30,
            background: cutweave_model::Color::BLACK,
            background_node: None,
            layers: vec![layer.clone(), layer],
            audio: Vec::new(),
        };
        q.quality().apply(&mut scene);
        assert!(scene.layers.iter().all(|l| l.request.proxy));
        Quality::FULL.apply(&mut scene);
        assert!(scene.layers.iter().all(|l| !l.request.proxy));

        // WP-2F.02: a frame rendered for an output tells each layer how large it is drawn.
        let proxies = Quality::new(Scale::Full, true);
        let drawn = |scene: &mut SceneDescription, output| {
            proxies.apply_for_output(scene, output);
            assert!(scene.layers.iter().all(|l| l.request.proxy));
            scene.layers[0].request.drawn_long_edge
        };
        // A 4K picture fitted into a 4K canvas.
        scene.width = 3840;
        scene.height = 2160;
        scene.layers[0].request.source_size = Some((3840, 2160));
        assert_eq!(drawn(&mut scene, (960, 540)), Some(960));
        assert_eq!(drawn(&mut scene, (1920, 1080)), Some(1920));
        assert_eq!(drawn(&mut scene, (961, 541)), Some(961));
        // Never more than the original has.
        assert_eq!(drawn(&mut scene, (7680, 4320)), Some(3840));
        // Scaled down to half, or mirrored: the size it is drawn at.
        scene.layers[0].transform.scale = Vec2::new(0.5, 0.5);
        assert_eq!(drawn(&mut scene, (1920, 1080)), Some(960));
        scene.layers[0].transform.scale = Vec2::new(-2.0, 2.0);
        assert_eq!(drawn(&mut scene, (960, 540)), Some(1920));
        scene.layers[0].transform.scale = Vec2::ONE;
        // Turned a quarter: the upright picture is pillarboxed, 1215 x 2160 of the canvas.
        scene.layers[0].request.rotation = 90;
        assert_eq!(drawn(&mut scene, (3840, 2160)), Some(2160));
        assert_eq!(drawn(&mut scene, (960, 540)), Some(540));
        scene.layers[0].request.rotation = 0;
        // A small picture fills the canvas but has only its own pixels.
        scene.layers[0].request.source_size = Some((640, 360));
        assert_eq!(drawn(&mut scene, (1920, 1080)), Some(640));
        // No probed size: nothing to say.
        scene.layers[0].request.source_size = None;
        assert_eq!(drawn(&mut scene, (1920, 1080)), None);
        // Originals only: no proxy, no size.
        scene.layers[0].request.source_size = Some((3840, 2160));
        Quality::FULL.apply_for_output(&mut scene, (960, 540));
        assert!(
            scene
                .layers
                .iter()
                .all(|l| !l.request.proxy && l.request.drawn_long_edge.is_none())
        );
        // The plain marking clears a size set before.
        proxies.apply_for_output(&mut scene, (960, 540));
        proxies.apply(&mut scene);
        assert!(
            scene
                .layers
                .iter()
                .all(|l| l.request.drawn_long_edge.is_none())
        );
    }
}
