// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows builds with the `ffmpeg` feature: put the FFmpeg DLLs the engine links next to the
//! binaries cargo writes (`target/<profile>/`).
//!
//! Windows looks for a DLL in the executable's folder first, then on `PATH`. Without this, every
//! binary that links FFmpeg (the engine, the CLI, the test executables) dies before `main` with
//! `STATUS_DLL_NOT_FOUND` (0xC0000135) whenever it is started from a shell, an app or an MCP
//! client that did not source `scripts/ffmpeg/env.sh`. Releases ship the same DLLs next to
//! `cutweave.exe` (packaging), so dev builds now match them. See docs/dev/ffmpeg.md.
//!
//! The DLLs are hard links into the bundle when the target folder is on the same volume (no copy,
//! no extra disk space), else copies. Nothing happens without the feature, on other systems, or
//! without `FFMPEG_DIR` (ffmpeg-sys-next then fails with its own message).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The libraries the engine links (S3 finding F6), by file-name prefix so a version bump of the
/// bundle (`avcodec-62.dll` → `avcodec-63.dll`) needs no change here.
const LINKED: [&str; 5] = [
    "avutil-",
    "avcodec-",
    "avformat-",
    "swscale-",
    "swresample-",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let windows = env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows");
    if env::var_os("CARGO_FEATURE_FFMPEG").is_none() || !windows {
        return;
    }
    let Some(bundle) = env::var_os("FFMPEG_DIR").map(PathBuf::from) else {
        return;
    };
    let Some(profile) = profile_dir() else {
        println!("cargo:warning=cannot place the FFmpeg DLLs: unexpected OUT_DIR layout");
        return;
    };
    let dlls = match linked_dlls(&bundle.join("bin")) {
        Ok(dlls) if !dlls.is_empty() => dlls,
        Ok(_) | Err(_) => {
            println!(
                "cargo:warning=no FFmpeg DLLs in {}; binaries need the bundle's bin/ on PATH",
                bundle.join("bin").display()
            );
            return;
        }
    };
    // Older cargo also runs test executables from `deps/`; the newer layout keeps them under
    // `build/`, where nextest finds the DLLs through env.sh's PATH as before.
    let deps = profile.join("deps");
    let dirs = if deps.is_dir() {
        vec![profile, deps]
    } else {
        vec![profile]
    };
    for dir in dirs {
        for dll in &dlls {
            let Some(name) = dll.file_name() else {
                continue;
            };
            let dest = dir.join(name);
            // A missing destination re-runs this script on the next build.
            println!("cargo:rerun-if-changed={}", dest.display());
            println!("cargo:rerun-if-changed={}", dll.display());
            if let Err(e) = place(dll, &dest) {
                println!("cargo:warning=could not place {}: {e}", dest.display());
            }
        }
    }
}

/// `target/<profile>` (or `target/<triple>/<profile>`), where cargo puts the binaries. `OUT_DIR`
/// is `<profile>/build/<package>/<hash>/out` (the layout cargo 1.100 uses) or
/// `<profile>/build/<package>-<hash>/out` (older cargo).
fn profile_dir() -> Option<PathBuf> {
    let out = PathBuf::from(env::var_os("OUT_DIR")?);
    [3, 2].into_iter().find_map(|depth| {
        let build = out.ancestors().nth(depth)?;
        (build.file_name()? == "build").then(|| build.parent().map(Path::to_path_buf))?
    })
}

fn linked_dlls(bin: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut dlls = Vec::new();
    for entry in fs::read_dir(bin)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let dll = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
        if dll && LINKED.iter().any(|p| lower.starts_with(p)) {
            dlls.push(path);
        }
    }
    dlls.sort();
    Ok(dlls)
}

/// Hard-links `src` to `dest`, or copies it. Leaves an identical `dest` alone, so a DLL that a
/// running engine holds open is not touched.
fn place(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let (Ok(a), Ok(b)) = (fs::metadata(src), fs::metadata(dest)) {
        if a.len() == b.len() && a.modified().ok() == b.modified().ok() {
            return Ok(());
        }
        fs::remove_file(dest)?;
    }
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::hard_link(src, dest).or_else(|_| fs::copy(src, dest).map(|_| ()))
}
