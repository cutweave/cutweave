// SPDX-License-Identifier: GPL-3.0-or-later
//! Small validation helpers shared by the command modules. Every helper returns
//! `CmdError::Invalid` with a JSON Pointer into the request params (arch §5.1).

use cutweave_commands::CmdError;
use cutweave_model::{Duration, FrameRate, Rounding, Time, TimeInput};

/// Longest accepted name of a project, sequence or track, in characters.
pub const MAX_NAME_CHARS: usize = 200;

/// A trimmed, non-empty name of at most [`MAX_NAME_CHARS`] characters without control
/// characters.
pub(crate) fn check_name(path: &str, name: &str) -> Result<String, CmdError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CmdError::invalid(path, "the name must not be empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CmdError::invalid(
            path,
            format!("the name must be at most {MAX_NAME_CHARS} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(CmdError::invalid(
            path,
            "the name must not contain control characters",
        ));
    }
    Ok(name.to_owned())
}

/// Whether `path` is absolute on any platform: `/x`, `\\server\share`, `C:\x` or `C:/x`.
pub(crate) fn is_absolute_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with('/')
        || path.starts_with("\\\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

/// A non-empty absolute path without control characters.
pub(crate) fn check_abs_path(pointer: &str, path: &str) -> Result<(), CmdError> {
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err(CmdError::invalid(
            pointer,
            "the path must be non-empty and free of control characters",
        ));
    }
    if !is_absolute_path(path) {
        return Err(CmdError::invalid(
            pointer,
            format!("`{path}` is not an absolute path"),
        ));
    }
    Ok(())
}

/// Resolves a time param against the owning sequence's frame rate.
pub(crate) fn resolve_time(
    pointer: &str,
    input: TimeInput,
    rate: FrameRate,
) -> Result<Time, CmdError> {
    input
        .resolve(Some(rate))
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// A strictly positive duration from a time param.
pub(crate) fn resolve_duration(
    pointer: &str,
    input: TimeInput,
    rate: FrameRate,
) -> Result<Duration, CmdError> {
    let t = resolve_time(pointer, input, rate)?;
    let d = Duration::try_from(t).map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    if d.is_zero() {
        return Err(CmdError::invalid(pointer, "the duration must be positive"));
    }
    Ok(d)
}

/// Resolves a time param and snaps it to the nearest frame boundary of the owning sequence
/// (ties round later). Edit commands carry frame-aligned times (arch §5.1); an input between
/// two frames is not an error but lands on the nearer one.
pub(crate) fn resolve_frame_time(
    pointer: &str,
    input: TimeInput,
    rate: FrameRate,
) -> Result<Time, CmdError> {
    let t = resolve_time(pointer, input, rate)?;
    rate.snap_to_frame(t, Rounding::Nearest)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))
}

/// Resolves a duration param and snaps it to a whole number of frames of the owning sequence
/// (nearest, ties later), at least one frame. A zero or negative input is still `Invalid`:
/// only a positive length that rounds below one frame becomes one frame.
pub(crate) fn resolve_frame_duration(
    pointer: &str,
    input: TimeInput,
    rate: FrameRate,
) -> Result<Duration, CmdError> {
    let d = resolve_duration(pointer, input, rate)?;
    let t = Time::from_flicks(d.flicks()).map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    let frames = rate.time_to_frame(t, Rounding::Nearest).max(1);
    let snapped = rate
        .frame_to_time(frames)
        .map_err(|e| CmdError::invalid(pointer, e.to_string()))?;
    Duration::try_from(snapped).map_err(|e| CmdError::invalid(pointer, e.to_string()))
}
