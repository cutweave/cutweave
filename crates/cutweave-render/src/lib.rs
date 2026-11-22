// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-render`: timeline evaluation, scene graph, render graph and wgpu passes (wasm-safe core).
//!
//! * WP-1F.01: [`evaluate`] turns a project snapshot, a sequence and a time into a
//!   [`SceneDescription`] (visible layers, frame requests, audio activations), the input of the
//!   compositor and the mixer (WP-1G). It is pure and deterministic (arch §8).
//! * WP-1F.02: the compositor. The wasm-safe core compiles a scene into a [`graph::RenderGraph`]
//!   (culling, occlusion, decode sharing, Merkle cache keys) using the [`color`] and
//!   [`geometry`] math, with an independent `f64` [`reference`] renderer and the §20
//!   [`metrics`]. The `gpu` module (feature `gpu`, native targets only) executes graphs with wgpu
//!   in linear-light `Rgba16Float`; the `golden` module (feature `golden`) stores per-adapter
//!   reference frames.
//!
//! Frames come from a [`FrameSource`] (the decoder pool / frame cache, WP-1D); until FFmpeg
//! decode lands, [`testcard::SyntheticSource`] serves generated test cards.

pub mod color;
mod eval;
pub mod frame;
pub mod geometry;
pub mod graph;
mod hash;
mod image;
pub mod metrics;
pub mod reference;
mod scene;
mod speed;
pub mod testcard;
#[cfg(test)]
mod tests;

#[cfg(all(
    feature = "gpu",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod gpu;

#[cfg(all(
    any(test, feature = "golden"),
    feature = "gpu",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
pub mod golden;

pub use eval::{EvalError, evaluate, evaluate_audio};
pub use frame::{ChromaSiting, FrameError, FrameId, FrameSource, PixelFormat, Plane, VideoFrame};
pub use graph::Target;
pub use hash::CacheKey;
pub use image::Image;
pub use scene::{
    AudioActivation, AudioSegment, BlendMode, Crop, EffectRef, FrameRequest, Layer, LayerTransform,
    SceneDescription,
};
pub use speed::Speed;
