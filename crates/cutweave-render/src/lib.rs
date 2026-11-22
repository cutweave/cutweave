// SPDX-License-Identifier: GPL-3.0-or-later
//! `cutweave-render`: timeline evaluation, scene graph, render graph and wgpu passes (wasm-safe core).
//!
//! WP-1F.01: [`evaluate`] turns a project snapshot, a sequence and a time into a
//! [`SceneDescription`] (visible layers, frame requests, audio activations), the input of the
//! compositor (WP-1F.02) and the mixer (WP-1G). It is pure and deterministic (arch §8).

mod eval;
mod scene;
mod speed;
#[cfg(test)]
mod tests;

pub use eval::{EvalError, evaluate, evaluate_audio};
pub use scene::{
    AudioActivation, AudioSegment, BlendMode, Crop, EffectRef, FrameRequest, Layer, LayerTransform,
    SceneDescription,
};
pub use speed::Speed;
