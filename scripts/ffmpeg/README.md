# FFmpeg shared bundle (WP-0A.06)

Cutweave links a pinned FFmpeg 8.1.x as **shared libraries** ([ADR-0003](../../docs/adr/0003-media-backend-ffmpeg.md), [architecture §21](../../docs/01-architecture.md), [FFmpeg build policy](../../docs/02-tech-stack.md)). CI builds it from source, together with every library linked into it, for three targets and publishes one archive per target plus the matching source archive. Developers download the newest one with `just ffmpeg-fetch`. Nothing here is committed: bundles and build trees live in the git-ignored `third_party/ffmpeg/`.

| File | Purpose |
|---|---|
| [versions.toml](versions.toml) | Every pin: FFmpeg tarball (version + sha256), each dependency (tag/branch + commit, or tarball + sha256) |
| [sources.py](sources.py) | Validates the pins, fetches and verifies all sources, writes the source tree / archive |
| [install-tools.sh](install-tools.sh) | Build tools per OS (apt in `ubuntu:22.04`, Homebrew, MSYS2 pacman) |
| [build-deps.sh](build-deps.sh) | Builds the dependencies as static, position-independent libraries |
| [build-ffmpeg.sh](build-ffmpeg.sh) | Configures and builds FFmpeg (shared), records `configure.txt` |
| [package.sh](package.sh) | Lays out the relocatable bundle, collects licenses, writes `BUILD_INFO.txt`, packs `.tar.zst` |
| [verify.sh](verify.sh) | Acceptance checks (below); runs the bundle's own `ffmpeg` |
| [fetch.sh](fetch.sh), [fetch.ps1](fetch.ps1) | Download the newest CI bundle for this OS (`just ffmpeg-fetch` runs `fetch.sh`), unless a bundle is already set up |
| [env.sh](env.sh), [env.ps1](env.ps1) | Source to build and run against the bundle: `FFMPEG_DIR`, libclang, the run-time library path ([docs/dev/ffmpeg.md](../../docs/dev/ffmpeg.md)) |
| [lib.sh](lib.sh) | Shared shell helpers and environment |

## What a bundle contains

`cutweave-ffmpeg-<version>-<os>-<arch>.tar.zst`, one per target:

| Target | Runner | Toolchain | Notes |
|---|---|---|---|
| `windows-x64` | `windows-latest` | MSYS2 UCRT64 GCC (preinstalled on the image) | Import libraries in both flavours: `avcodec.lib` (MSVC `lib.exe`) and `libavcodec.dll.a` (MinGW). The GCC runtime is linked statically; a MinGW runtime DLL that still slips in is bundled and listed in `licenses/mingw-runtime.txt` |
| `macos-arm64` | `macos-latest` | Apple clang | `MACOSX_DEPLOYMENT_TARGET=13.0` (ADR-0014 proposal); libraries are `@rpath/libavX.N.dylib` |
| `linux-x64` | `ubuntu:22.04` container | GCC 11 | glibc symbol versions <= 2.35 ([check-glibc.sh](../check-glibc.sh)); `$ORIGIN` rpaths |

```text
cutweave-ffmpeg-8.1.3-linux-x64/
  bin/            ffmpeg, ffprobe (Windows: also the avcodec-62.dll ... DLLs)
  lib/            libav*.so / .dylib, or avcodec.lib + libavcodec.dll.a; lib/pkgconfig/*.pc (relocatable)
  include/        FFmpeg headers
  licenses/       license texts of FFmpeg and of every library linked into it
  source/         SOURCES.txt, versions.toml, the build scripts, README.txt (where the full source is)
  configure.txt   the exact configure command line
  BUILD_INFO.txt  configure line, toolchain, components, ffmpeg -version/-buildconf/-hwaccels/-encoders/-filters
```

The configure line uses `--disable-autodetect`, so it is the complete feature list and configure fails if anything requested is missing. Enabled everywhere: `--enable-gpl --enable-version3 --enable-shared`, libx264, libx265 (8/10/12-bit), libsvtav1, libvpx (VP8/VP9, high bit depth), libopus, libmp3lame, libvidstab (resolution R6), libzimg, libsoxr, libass, plus libdav1d (software AV1 decoding) and libfreetype/libfribidi/libharfbuzz (already needed by libass; adds `drawtext`). Native `prores_ks`, `dnxhd` and `gif` encoders are part of FFmpeg. Deliberately absent: TLS (no `https://` inputs), bzip2/lzma, SDL (`ffplay`), Vulkan, and anything non-free. Hardware acceleration:

| OS | Enabled |
|---|---|
| Windows | `d3d11va`, `d3d12va`, `dxva2`, Media Foundation (`h264_mf`, `hevc_mf`), NVENC/NVDEC/CUVID (ffnvcodec), AMF, QSV (libvpl) |
| macOS | VideoToolbox, AudioToolbox |
| Linux | VA-API (DRM), VDPAU, libdrm, NVENC/NVDEC/CUVID (ffnvcodec), QSV (libvpl) |

GPU drivers and runtimes (NVIDIA, AMD AMF, Intel VPL, VA-API drivers) are loaded at runtime from the user's system. The NVENC/NVDEC headers are SDK 12.2, which needs NVIDIA driver 551.76+ (Windows) or 550.54+ (Linux). On Linux the bundle links these system libraries dynamically: glibc, libstdc++, zlib, libva, libva-drm, libvdpau, libX11, libdrm, fontconfig. Everything else is static inside the FFmpeg libraries.

## Fetching the CI bundle

```sh
gh auth login            # once; any account that can read the repository
just ffmpeg-fetch        # or: bash scripts/ffmpeg/fetch.sh, pwsh scripts/ffmpeg/fetch.ps1
```

This finds the newest unexpired artifact named `cutweave-ffmpeg-<version>-<os>-<arch>.tar.zst` (version from `versions.toml`) built from `main`, downloads it with `gh run download`, unpacks it to `third_party/ffmpeg/<name>/` and prints the `FFMPEG_DIR` to export (also written to `third_party/ffmpeg/ffmpeg.env`). Running it again is a no-op until a newer run exists. Options: `--run <id>` (a specific workflow run), `--branch <name>`, `--repo <owner/name>` (forks; or `CUTWEAVE_FFMPEG_REPO`), `--update`, `--force`.

A bundle that is already set up wins, and nothing is downloaded: `$FFMPEG_DIR`, then the `ffmpeg.env` of this checkout, then that of the main checkout (git worktrees share it). `--update`, `--run` and `--force` go to CI instead. This is how a dev bundle (for example a BtbN 8.1 build while no CI bundle is published) is used; see [docs/dev/ffmpeg.md](../../docs/dev/ffmpeg.md).

Using the bundle:

- **Rust (`ffmpeg-sys-next`)**: set `FFMPEG_DIR`; the build script links `$FFMPEG_DIR/lib` and uses `$FFMPEG_DIR/include`. MSVC targets link `avcodec.lib` etc., `*-pc-windows-gnu` targets `libavcodec.dll.a`.
- **Running** anything linked against it needs the libraries on the loader path: `bin/` on `PATH` (Windows), `lib/` in `DYLD_FALLBACK_LIBRARY_PATH` (macOS) or `LD_LIBRARY_PATH` (Linux). The bundle's own `ffmpeg`/`ffprobe` work without that.
- **pkg-config**: `PKG_CONFIG_PATH=$FFMPEG_DIR/lib/pkgconfig` (the `.pc` files are relative to the bundle). Only shared linking is supported.

Workflow artifacts expire after 90 days: if `ffmpeg-fetch` finds nothing, run the workflow (Actions tab, "FFmpeg bundle", or `gh workflow run ffmpeg.yml`). For quick experiments Homebrew or distro `libav*-dev` packages also work ([dev process §2](../../docs/06-dev-process.md)), but CI and releases use this bundle only.

## How CI builds it

[`.github/workflows/ffmpeg.yml`](../../.github/workflows/ffmpeg.yml) runs only on `workflow_dispatch` (Actions tab or `gh workflow run ffmpeg.yml`), to conserve Actions minutes: dispatch it after changing the workflow, `scripts/ffmpeg/**` or `scripts/check-glibc.sh`.

1. **`sources`** validates `versions.toml`, downloads every tarball (sha256-checked, mirrors allowed), clones every git dependency and checks that its tag still points at, or its branch still contains, the pinned commit, then archives it with `git archive`. The result is uploaded as `cutweave-ffmpeg-<version>-source.tar.zst`.
2. **`build (linux-x64)`, `build (macos-arm64)`, `build (windows-x64)`** unpack that same source archive (so the published source is exactly what was compiled), install the tools, build the static dependencies (cached per OS, keyed on the pins, the build scripts and the compiler version; the `clean` input skips the cache), build FFmpeg, package, verify and upload `cutweave-ffmpeg-<version>-<os>-<arch>.tar.zst`. On failure the configure/meson/CMake logs are uploaded.

**Acceptance checks** ([verify.sh](verify.sh), run on the packed archive): bundle layout and import libraries; `ffmpeg -version` reports the pinned version and `--enable-gpl`; `configure.txt` and `ffmpeg -buildconf` contain every required `--enable-*` flag; `-encoders` lists `libx264 libx265 libsvtav1 libvpx libvpx-vp9 libopus libmp3lame prores_ks dnxhd gif` plus the per-OS hardware encoders; `-decoders` lists `libdav1d` (and CUVID decoders on Windows/Linux); `-filters` lists `vidstabdetect vidstabtransform zscale subtitles ass drawtext`; `-hwaccels` lists the per-OS methods; dynamic linkage is limited to the system libraries above (and glibc <= 2.35 on Linux).

**Windows toolchain.** FFmpeg's supported Windows build is MinGW; building it and all dependencies with MSVC would need per-library project files. So the build uses the MSYS2 that is preinstalled on the runner (no third-party setup action) and puts MSVC's `lib.exe` (found with `vswhere`) on `PATH`, which makes FFmpeg emit MSVC-native import libraries next to the MinGW ones. MSYS2 packages are rolling, not pinned; the exact compiler is recorded in `BUILD_INFO.txt` and is part of the dependency cache key.

## Building locally from source

The CI steps work on a developer machine; outputs go to `third_party/ffmpeg/build/` (`CW_WORK` overrides it, `CW_JOBS` sets parallelism). Expect 30-90 minutes.

```sh
python3 scripts/ffmpeg/sources.py fetch --os linux   # Python 3.11+, curl, git; or unpack a CI source archive into third_party/ffmpeg/
bash scripts/ffmpeg/install-tools.sh linux           # see below per OS
bash scripts/ffmpeg/build-deps.sh                    # or a subset in order, e.g. build-deps.sh x265
bash scripts/ffmpeg/build-ffmpeg.sh
bash scripts/ffmpeg/package.sh
bash scripts/ffmpeg/verify.sh third_party/ffmpeg/build/dist/cutweave-ffmpeg-8.1.3-linux-x64.tar.zst
```

- **Linux:** build inside `ubuntu:22.04` to keep the glibc 2.35 floor, for example `docker run --rm -it -v "$PWD":/src -w /src ubuntu:22.04`, then run the steps above as root (`install-tools.sh` uses apt and pip). The sources step can run on the host.
- **macOS (arm64):** Xcode command-line tools and Homebrew; `install-tools.sh macos` installs only missing tools.
- **Windows:** an MSYS2 **UCRT64** shell (`install-tools.sh windows` uses pacman). Visual Studio or the Build Tools with the C++ workload provide `lib.exe` for MSVC import libraries; without them `dlltool` generates them.

The scripts never download anything themselves except in `sources.py` (pinned, verified) and `install-tools.sh` (package managers).

## Updating a pin

1. Edit [versions.toml](versions.toml): new version and the new sha256 (tarballs; from the release page, or `sha256sum` of the downloaded file) or the new tag and its full commit id (`git ls-remote <url> refs/tags/<tag>`; for annotated tags use the `^{}` line).
2. `python3 scripts/ffmpeg/sources.py check`, then dispatch the workflow on the branch (`gh workflow run ffmpeg.yml --ref <branch>`): it builds and verifies all three bundles and fails if a hash or commit does not match.
3. Check the per-OS `BUILD_INFO.txt` in the artifacts, and the configure flags if FFmpeg's options changed.

## Licensing obligations

The bundle is **GPL-3.0-or-later** as a whole: FFmpeg is configured with `--enable-gpl --enable-version3`, and x264, x265 and vid.stab (GPL-2.0-or-later) are combined under version 3. That is compatible with Cutweave's GPL-3.0-or-later ([architecture §21](../../docs/01-architecture.md)). `--enable-nonfree` and non-free components (fdk-aac, the CUDA SDK/NPP) are never enabled: they would make the bundle undistributable.

| Component | License |
|---|---|
| FFmpeg | LGPL-2.1-or-later, GPL-3.0-or-later as built |
| x264, x265, vid.stab | GPL-2.0-or-later |
| SVT-AV1 | BSD-3-Clause-Clear + AOMedia Patent License 1.0 |
| libvpx, Opus | BSD-3-Clause |
| dav1d | BSD-2-Clause |
| LAME | LGPL-2.0-or-later |
| soxr, FriBidi | LGPL-2.1-or-later |
| zimg | WTFPL |
| libass | ISC |
| FreeType | FTL or GPL-2.0-or-later |
| HarfBuzz, nv-codec-headers, AMF headers, libvpl | MIT |
| zlib (Windows only) | Zlib |

Anyone who **redistributes** a bundle, or a Cutweave build that contains its libraries (installers, release assets: the P7 release work), must:

1. Ship the license texts (`licenses/`) and keep the notices in `BUILD_INFO.txt` and `source/`.
2. Provide the **complete corresponding source**: the `cutweave-ffmpeg-<version>-source.tar.zst` from the same workflow run (named, with its sha256, in `source/README.txt` and `BUILD_INFO.txt`). It contains every upstream source archive and the build scripts. Attach it to the same GitHub Release as the binaries, or otherwise keep it available for as long as the binaries are offered (GPLv3 §6). Workflow artifacts expire after 90 days, so a link to a workflow run is not a durable source offer.
3. Publish the exact configure line (`configure.txt`) with every release, as architecture §21 requires.
4. Impose no further restrictions (for example, no terms that forbid reverse engineering of these libraries).

The Linux system libraries the bundle links against (glibc, libva, fontconfig, ...) are not part of it; they fall under the GPL's system-library exception. If a MinGW runtime DLL is ever bundled on Windows, `licenses/mingw-runtime.txt` records it (GCC runtime library exception). Codec patents (H.264/HEVC/AAC) are a separate question from copyright licensing: Cutweave prefers OS and hardware codecs and seeks legal review before 1.0 (architecture §21). Developers who only fetch a bundle for local builds have no redistribution obligations.

## Not covered here

WBS items for the integrator: publishing the bundles as GitHub Release assets with a sha256 manifest (`bundles.toml`), `cargo xtask fetch-ffmpeg`, restoring the bundle in other CI jobs, and `just setup` fetching it ([WP-0A.06](../../docs/wbs/P0-foundations.md)). The source-offer page is P7.
