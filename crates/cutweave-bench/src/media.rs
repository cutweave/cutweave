// SPDX-License-Identifier: GPL-3.0-or-later
//! The source files of a fixture, made on this machine with the FFmpeg bundle: four 1080p
//! test-pattern videos and two tones, [`SOURCE_SECS`](crate::fixture::SOURCE_SECS) long. Never
//! committed; a benchmark that needs them skips when there is no `ffmpeg`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::BenchError;
use crate::fixture::FixtureManifest;

/// The bundle's `ffmpeg` (`$FFMPEG_DIR/bin`), else the one on `PATH`; `None` when neither runs.
#[must_use]
pub fn ffmpeg() -> Option<PathBuf> {
    let exe = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let candidate = std::env::var_os("FFMPEG_DIR")
        .map(|d| PathBuf::from(d).join("bin").join(&exe))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from(&exe));
    Command::new(&candidate)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        .then_some(candidate)
}

/// Whether every source file of the fixture in `folder` exists.
#[must_use]
pub fn present(folder: &Path, manifest: &FixtureManifest) -> bool {
    manifest
        .media
        .iter()
        .all(|m| folder.join("media").join(&m.file).is_file())
}

/// Makes the missing source files of the fixture in `folder`. `Ok(false)`: there is no
/// `ffmpeg`, nothing was made.
pub fn synthesize(folder: &Path, manifest: &FixtureManifest) -> Result<bool, BenchError> {
    if present(folder, manifest) {
        return Ok(true);
    }
    let Some(tool) = ffmpeg() else {
        return Ok(false);
    };
    let secs = manifest.source_secs.to_string();
    for (n, m) in manifest.media.iter().enumerate() {
        let out = folder.join("media").join(&m.file);
        if out.is_file() {
            continue;
        }
        let mut cmd = Command::new(&tool);
        cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i"]);
        if m.video {
            // The same moving pattern in four hues: real motion to decode, small files.
            cmd.arg(format!("testsrc2=size=1920x1080:rate=30,hue=h={}", n * 90));
            cmd.args(["-t", &secs, "-c:v", "libx264", "-preset", "veryfast"]);
            cmd.args(["-crf", "36", "-g", "30", "-pix_fmt", "yuv420p"]);
        } else {
            cmd.arg(format!("sine=frequency={}:sample_rate=48000", 220 * (n + 1)));
            cmd.args(["-t", &secs, "-ac", "2", "-c:a", "pcm_s16le"]);
        }
        let status = cmd
            .arg(&out)
            .stdin(Stdio::null())
            .status()
            .map_err(|e| BenchError::io("run ffmpeg for", &out, e))?;
        if !status.success() {
            let _ = std::fs::remove_file(&out);
            return Err(BenchError::Other(format!(
                "ffmpeg could not make {} ({status})",
                out.display()
            )));
        }
    }
    Ok(true)
}
