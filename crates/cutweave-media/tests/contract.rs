// SPDX-License-Identifier: GPL-3.0-or-later
//! The backend contract (`cutweave_media::testing::contract`) against every backend this build
//! has: `FakeBackend` always, `FfmpegBackend` with `--features ffmpeg` (docs/dev/ffmpeg.md).
#![allow(clippy::expect_used)] // a backend that cannot start fails the test

use cutweave_media::{FakeBackend, backend_contract};

backend_contract!(fake, FakeBackend::new());

#[cfg(feature = "ffmpeg")]
backend_contract!(
    ffmpeg,
    cutweave_media::FfmpegBackend::new().expect("the loaded FFmpeg matches the build")
);
