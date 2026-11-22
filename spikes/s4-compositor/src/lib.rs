//! Spike S4 (WP-0F.01): scene graph → render graph (DAG of passes) → wgpu executor.
//!
//! Throwaway spike code (docs/wbs/P0-foundations.md). Only the learnings carry forward
//! into `cutweave-render`; see `README.md` for the design and findings.
//!
//! Pipeline (architecture §8):
//!
//! ```text
//! timeline::demo_scene(frame) ─► scene::Scene ─► graph::build ─► graph::RenderGraph
//!                                     │                               │
//!                                     ▼                               ▼
//!                         cpu_ref::render (f64 reference)     gpu::Renderer::render (wgpu)
//!                                     └──────────► metrics (PSNR / SSIM / ΔE2000) ◄──┘
//! ```

pub mod color;
pub mod cpu_ref;
pub mod error;
pub mod export;
pub mod golden;
pub mod gpu;
pub mod graph;
pub mod hash;
pub mod image;
pub mod lut;
pub mod metrics;
pub mod patterns;
pub mod scene;
pub mod source;
pub mod timeline;

pub use error::{Error, Result};
