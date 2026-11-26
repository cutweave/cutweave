// SPDX-License-Identifier: GPL-3.0-or-later
//! Canvas presets for new sequences (WP-2B.01): the aspect ratios the editor offers, each with
//! a canvas size, and the frame rates it offers. Plain data (L0): a plugin pack or a later
//! version adds rows, nothing reads them by index.
//!
//! The sizes line up with the export presets of `plugins/presets-core` (YouTube 1080p and 4K,
//! Shorts, Reels, Stories and TikTok at 1080x1920, the Instagram feed at 1080x1350 and
//! 1080x1080), so a sequence made from a preset exports without scaling.

use serde::Serialize;

use crate::FrameRate;

/// One canvas preset: a named aspect ratio with a pixel size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema, specta::Type)]
pub struct AspectPreset {
    /// Stable ID, the value of `preset` in `sequence.create`: the ratio (`"9:16"`), with a
    /// `-4k` suffix for the larger 16:9 canvas.
    pub id: &'static str,
    /// Aspect width (the `16` of 16:9).
    pub aspect_w: u32,
    /// Aspect height (the `9` of 16:9).
    pub aspect_h: u32,
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// What the shape is used for.
    pub use_for: &'static str,
}

/// The aspect presets, in the order the UI lists them.
pub const ASPECT_PRESETS: &[AspectPreset] = &[
    AspectPreset {
        id: "16:9",
        aspect_w: 16,
        aspect_h: 9,
        width: 1920,
        height: 1080,
        use_for: "YouTube, long-form, X landscape",
    },
    AspectPreset {
        id: "16:9-4k",
        aspect_w: 16,
        aspect_h: 9,
        width: 3840,
        height: 2160,
        use_for: "YouTube 4K",
    },
    AspectPreset {
        id: "9:16",
        aspect_w: 9,
        aspect_h: 16,
        width: 1080,
        height: 1920,
        use_for: "Shorts, Reels, Stories, TikTok",
    },
    AspectPreset {
        id: "1:1",
        aspect_w: 1,
        aspect_h: 1,
        width: 1080,
        height: 1080,
        use_for: "Instagram feed square",
    },
    AspectPreset {
        id: "4:5",
        aspect_w: 4,
        aspect_h: 5,
        width: 1080,
        height: 1350,
        use_for: "Instagram feed portrait",
    },
    AspectPreset {
        id: "21:9",
        aspect_w: 21,
        aspect_h: 9,
        width: 2560,
        height: 1080,
        use_for: "Ultrawide",
    },
];

/// The frame rates the editor offers (any rate a sequence accepts can still be typed).
pub const FRAME_RATE_PRESETS: [FrameRate; 5] = [
    FrameRate::FPS_24,
    FrameRate::FPS_25,
    FrameRate::FPS_30,
    FrameRate::FPS_50,
    FrameRate::FPS_60,
];

/// The aspect preset with this ID, if any.
#[must_use]
pub fn aspect_preset(id: &str) -> Option<&'static AspectPreset> {
    ASPECT_PRESETS.iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_CANVAS_SIDE, Project};

    #[test]
    fn ids_are_unique_and_sizes_valid() {
        for (i, p) in ASPECT_PRESETS.iter().enumerate() {
            assert!(ASPECT_PRESETS[..i].iter().all(|q| q.id != p.id), "{}", p.id);
            assert!((1..=MAX_CANVAS_SIDE).contains(&p.width));
            assert!((1..=MAX_CANVAS_SIDE).contains(&p.height));
            // The pixel size has the preset's ratio.
            assert_eq!(
                u64::from(p.width) * u64::from(p.aspect_h),
                u64::from(p.height) * u64::from(p.aspect_w),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn every_preset_makes_a_valid_format() {
        for p in ASPECT_PRESETS {
            for rate in FRAME_RATE_PRESETS {
                let mut project = Project::new("preset");
                project.settings.default_format.width = p.width;
                project.settings.default_format.height = p.height;
                project.settings.default_format.frame_rate = rate;
                assert!(project.validate().is_ok(), "{} @ {rate}", p.id);
            }
        }
    }

    #[test]
    fn lookup() {
        assert_eq!(
            aspect_preset("9:16").map(|p| (p.width, p.height)),
            Some((1080, 1920))
        );
        assert!(aspect_preset("2:3").is_none());
    }
}
