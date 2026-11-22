// SPDX-License-Identifier: GPL-3.0-or-later
//! Spike S4b (WP-0G.01): programmable compositions.
//!
//! Throwaway spike code (docs/wbs/P0-foundations.md). Only the learnings carry forward into
//! `cutweave-compose` and `packages/compose-runtime`; see `README.md`.
//!
//! ```text
//! lower-third.tsx ─► transpile (oxc: TS strip + JSX → jsx() calls, source map, policy checks)
//!                       │
//!                       ▼
//!   runtime (rquickjs / QuickJS-ng sandbox: no I/O, no clock, seeded RNG, memory + time limits)
//!     defineComposition(...).render({frame, fps, props, size}) ─► nodes (JSON)
//!                       │
//!                       ▼
//!   lower (nodes → s4 Scene: groups sized to their content, text → s4 Text or placeholders)
//!                       │
//!                       ▼
//!   s4::gpu::Renderer (wgpu, linear light) ─► PNG
//! ```

pub mod adversarial;
pub mod assets;
pub mod diagnostics;
pub mod lower;
pub mod render;
pub mod runtime;
pub mod scene;
pub mod stats;
pub mod transpile;

pub use diagnostics::{ComposeError, Diagnostic, ErrorKind};
pub use runtime::{Composition, FrameEvaluator, Limits};
pub use transpile::{Compiled, transpile};

/// The §12 lower-third example.
pub const LOWER_THIRD_TSX: &str = include_str!("../examples/lower-third.tsx");
/// Props for the lower-third example.
pub const LOWER_THIRD_PROPS: &str = include_str!("../examples/lower-third.props.json");
/// 200-node stress composition.
pub const STRESS_TSX: &str = include_str!("../examples/stress-200.tsx");
