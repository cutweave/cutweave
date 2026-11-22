// SPDX-License-Identifier: GPL-3.0-or-later
//! Composition frame → S4 scene → wgpu → PNG.

use std::time::Instant;

use s4::gpu::{AdapterChoice, FrameStats, Renderer};
use s4::image::{Image, cicp_for, encode_png};
use s4::scene::Scene;

use crate::assets::AssetSources;
use crate::diagnostics::ComposeError;
use crate::lower::{LowerOptions, TextMode, lower};
use crate::scene::FrameOutput;

/// Text handling requested by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextChoice {
    /// Native S4 text if the renderer supports it (WP-0F.02), else placeholders.
    Auto,
    Native,
    Placeholder,
}

impl TextChoice {
    /// `auto`, `native` or `placeholder`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "native" => Some(Self::Native),
            "placeholder" => Some(Self::Placeholder),
            _ => None,
        }
    }
}

/// A rendered composition frame.
pub struct RenderedFrame {
    pub image: Image,
    pub png: Vec<u8>,
    pub scene: Scene,
    /// What text actually rendered as.
    pub text: TextMode,
    pub gpu: FrameStats,
    pub lower_us: f64,
}

fn s4_err(e: &s4::Error) -> ComposeError {
    ComposeError::engine(format!("S4 compositor: {e}"))
}

/// Headless renderer for composition frames.
pub struct FrameRenderer {
    renderer: Renderer,
    assets: AssetSources,
    text: TextChoice,
    /// Set once `Auto` has found out whether the S4 executor renders text.
    native_text_unsupported: bool,
    pub options: LowerOptions,
}

impl FrameRenderer {
    /// Creates a renderer on `adapter` for a `width × height` canvas.
    pub fn new(
        adapter: &AdapterChoice,
        width: u32,
        height: u32,
        text: TextChoice,
    ) -> Result<Self, ComposeError> {
        Ok(Self {
            renderer: Renderer::new(adapter).map_err(|e| s4_err(&e))?,
            assets: AssetSources::new(width, height).map_err(|e| s4_err(&e))?,
            text,
            native_text_unsupported: false,
            options: LowerOptions {
                width,
                height,
                ..LowerOptions::default()
            },
        })
    }

    /// Adapter description.
    #[must_use]
    pub fn adapter(&self) -> String {
        let a = self.renderer.adapter();
        format!("{} ({}, {})", a.name, a.backend, a.device_type)
    }

    /// Asset provider (for lowering without rendering).
    #[must_use]
    pub fn assets(&self) -> &AssetSources {
        &self.assets
    }

    fn mode(&self) -> TextMode {
        match self.text {
            TextChoice::Native => TextMode::Native,
            TextChoice::Placeholder => TextMode::Placeholder,
            TextChoice::Auto if self.native_text_unsupported => TextMode::Placeholder,
            TextChoice::Auto => TextMode::Native,
        }
    }

    /// Lowers and renders one frame; `media_frame` drives the optional `under` media.
    pub fn render(
        &mut self,
        out: &FrameOutput,
        media_frame: i64,
    ) -> Result<RenderedFrame, ComposeError> {
        loop {
            let mode = self.mode();
            let t = Instant::now();
            let opts = LowerOptions {
                text: mode,
                under_frame: media_frame,
                ..self.options.clone()
            };
            let scene = lower(out, &opts, &self.assets)?;
            let lower_us = t.elapsed().as_secs_f64() * 1e6;
            match self.renderer.render_scene(&scene, &self.assets) {
                Ok(frame) => {
                    let png = encode_png(&frame.image, Some(cicp_for(&scene.output)))
                        .map_err(|e| s4_err(&e))?;
                    return Ok(RenderedFrame {
                        image: frame.image,
                        png,
                        scene,
                        text: mode,
                        gpu: frame.stats,
                        lower_us,
                    });
                }
                Err(s4::Error::Unsupported(msg))
                    if self.text == TextChoice::Auto
                        && mode == TextMode::Native
                        && msg.contains("text") =>
                {
                    tracing_note(&msg);
                    self.native_text_unsupported = true;
                }
                Err(e) => return Err(s4_err(&e)),
            }
        }
    }
}

fn tracing_note(msg: &str) {
    // Spike: no tracing subscriber; a one-time stderr note is enough.
    eprintln!("note: S4 has no text pass yet ({msg}); rendering text as placeholders");
}
