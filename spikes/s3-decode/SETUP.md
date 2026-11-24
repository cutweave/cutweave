# S3 setup: building and running against FFmpeg 8.1 (feeds WP-1D.01)

How `ffmpeg-sys-next` finds, binds and links FFmpeg, and what a process needs at run time. Only the
Windows recipe is tested (WP-0E.01). The macOS and Linux sections are the expected recipe for
WP-0E.03 and still need a run.

## What `ffmpeg-sys-next` 9.1.1 does at build time

When `FFMPEG_DIR` is set (the path the spike and `cutweave-media` use), the build script does four
things:

1. **Link search.** It adds `$FFMPEG_DIR/lib`, or `lib/x64` / `lib/amd64` / `lib/arm64` if one of
   those exists.
2. **Linking.** It links `avcodec`, `avformat`, `avutil` and one more library per enabled cargo
   feature (`swscale`, `swresample`, `avfilter`, `avdevice`) as `dylib`. On MSVC that resolves to
   the **import libraries** `avcodec.lib` etc. With `--target *-windows-gnu` it would use
   `libavcodec.dll.a`.
3. **Bindings.** It runs **bindgen** on `$FFMPEG_DIR/include`. This needs libclang and produces a
   9 MB `bindings.rs` (a 70 MB rlib).
4. **Version check.** It compiles and runs a small C program (`check.c`, built with `cc`, so
   `cl.exe` on Windows) that reads the version macros. Each detected version becomes a cfg:
   `ffmpeg_8_0`, `ffmpeg_8_1` (lavc ≥ 62.28) and so on, up to `ffmpeg_9_1`. **ffmpeg-next 9.1.0
   compiles its FFmpeg 8 code paths from these cfgs.** For example, stream side data comes from
   `codecpar->coded_side_data` because `AVStream.side_data` is gone in 8.0. The C compiler only
   includes headers; it does not link FFmpeg.

Without `FFMPEG_DIR` it falls back to vcpkg (Windows), then pkg-config. pkg-config works with the
bundle's `lib/pkgconfig` (`PKG_CONFIG_PATH`), but `FFMPEG_DIR` is simpler and is the same on every
OS.

The `build` feature (FFmpeg compiled from source by the build script) is **not** used. It would
download FFmpeg during the build, which breaks "no network in builds/tests" and the pinned-bundle
policy (arch §21).

## Windows 11 x64 (tested)

| Need | Used here |
|---|---|
| Rust | 1.100.0 (`rust-toolchain.toml`), target `x86_64-pc-windows-msvc` |
| MSVC Build Tools | `cl.exe` (version check) and `link.exe` |
| LLVM / libclang | `C:\Program Files\LLVM\bin\libclang.dll` (bindgen) |
| FFmpeg dev bundle | BtbN `ffmpeg-n8.1-latest-win64-gpl-shared` = **8.1.3-39-g660b5aa7db** (2026-11-23), lavc 62.28.103, built with MinGW GCC 16.2. Layout: `include/`, `lib/*.lib` (MSVC import libs) + `lib/*.dll.a` + `*.def`, `bin/*.dll`, `bin/ffmpeg.exe`, `bin/ffprobe.exe`. The CI bundle from `just ffmpeg-fetch` (WP-0A.06) has the same layout |
| Python 3 | Only for `gen_media.py` |

Environment (the scripts `env.ps1` and `env.sh` set it; they read
`third_party/ffmpeg/ffmpeg.env` from the main checkout, which also works from git worktrees):

```powershell
$env:FFMPEG_DIR    = "D:/projects/Vidai/third_party/ffmpeg/btbn-n8.1-win64-gpl-shared"
$env:LIBCLANG_PATH = "C:/Program Files/LLVM/bin"      # bindgen; often found without it
$env:PATH          = "$env:FFMPEG_DIR/bin;$env:PATH"   # run time only: the DLLs
$env:CARGO_BUILD_JOBS = 4                               # shared machine
```

Build and run:

```powershell
. .\env.ps1
python gen_media.py              # about 8 min, about 590 MB into media/ (git-ignored)
cargo build --release
cargo test                        # unit tests: no media needed, but the DLLs must be on PATH
cargo test -- --ignored           # media-dependent harness tests (needs gen_media.py first)
.\target\release\s3.exe all       # info + verify + audio + robust + fuzz
.\target\release\s3.exe bench
```

Observations:

- **First build: about 6 minutes**, on a 16-thread machine shared with other builds and limited
  to `CARGO_BUILD_JOBS=4`. bindgen over the FFmpeg headers dominates. sccache caches the result,
  and later builds take seconds. Only `FFMPEG_DIR` changes trigger a rebuild
  (`rerun-if-env-changed`). Expect the same cost in CI unless the target dir or sccache is cached.
- **MSVC warnings** C4244 from `libavutil/common.h` appear during the version check. They are
  harmless.
- **The DLLs must be on the loader path at start-up**, for tests too. Without them the process
  dies before `main` with `STATUS_DLL_NOT_FOUND` (0xC0000135). Git Bash reports this misleadingly
  as `api-ms-win-crt-heap-l1-1-0.dll: cannot open shared object file`. `cargo test` of any crate
  that links `cutweave-media` with the `ffmpeg` feature therefore needs `PATH` set. Alternatively,
  a build script can copy the DLLs next to the test executables. Releases put them next to
  `cutweave.exe`, where the loader searches first.
- **Run-time closure for decode:** `avcodec-62.dll`, `avformat-62.dll`, `avutil-60.dll` **and
  `swresample-6.dll`**. avcodec imports swresample even though the spike never calls it.
  Everything else they import is a Windows system DLL or the UCRT (`api-ms-win-crt-*`, present
  on Windows 10+). The BtbN build links its codec libraries statically into these DLLs:
  `avcodec-62.dll` alone is 122 MB, because it also contains aom, rav1e, vvenc, libplacebo and
  more. The WP-0A.06 bundle is smaller (fewer components).
- `ffmpeg-next`'s default features would also link `avfilter`, `avdevice`, `swscale` and
  `swresample`. The spike uses `default-features = false, features = ["codec", "format"]`, which
  links only avcodec, avformat and avutil. `cutweave-media` will want `software-scaling`
  (thumbnails, RGBA for agent frames) and `software-resampling` (audio), but probably not
  `device`.

## Hardware decode and GPU upload (WP-0E.02)

Nothing extra is linked for hardware decode: the hwaccels are inside the FFmpeg DLLs, and the
spike reaches them through `ffmpeg-sys-next`'s raw bindings (`hwcontext.h`, `hwcontext_d3d11va.h`
are in its bindings). What a machine needs is a GPU driver:

| Hwaccel (`--hw`) | Needs | Device argument |
|---|---|---|
| `d3d11va[:N]`, `d3d11va:nvidia` | Windows 8+, any DX11 GPU driver | DXGI adapter index, or a vendor (`nvidia`, `amd`, `intel`: FFmpeg's `vendor_id` option). Prefer the vendor: indexes flip inside a process on hybrid laptops (README H7) |
| `d3d12va[:N]` | Windows 10 2004+, DX12 video driver | DXGI adapter index |
| `cuda[:N]` | NVIDIA driver (`nvcuda.dll`, `nvcuvid.dll`); no CUDA toolkit | CUDA device index |
| `vulkan[:N]` | Vulkan driver with `VK_KHR_video_decode_*` | Vulkan physical device index |
| `videotoolbox` | macOS | none |
| `vaapi[:/dev/dri/renderDN]` | Linux, Mesa `va` driver (`radeonsi`, `iHD`) or `nvidia-vaapi-driver`; the user in group `render` | DRM render node |

The wgpu side (`gpu` feature, default on) uses the same pinned `wgpu` 30.0.2 as `cutweave-render`.
The `zero-copy` feature adds the `windows` 0.62 crate (D3D11/D3D12/DXGI bindings; same line as
wgpu-hal's, already in the root lockfile). Both resolve offline from the registry cache.

```powershell
cargo build --release                       # path (a): hw decode + CPU upload (default features)
cargo build --release --features zero-copy  # + path (b) experiment (Windows)
.\target\release\s3.exe hwinfo              # which hwaccel devices open, wgpu adapters
```

## macOS arm64 (expected; run in WP-0E.03)

```sh
just ffmpeg-fetch                 # or: brew install ffmpeg (8.1); FFMPEG_DIR=$(brew --prefix ffmpeg)
. ./env.sh                        # FFMPEG_DIR + DYLD_FALLBACK_LIBRARY_PATH=$FFMPEG_DIR/lib
xcode-select --install            # clang/libclang for bindgen and the version check
cargo build --release && ./target/release/s3 all
```

The CI bundle's dylibs use `@rpath/libavX.N.dylib`. A product binary needs an `LC_RPATH` such as
`@executable_path/../Frameworks`, through `cargo:rustc-link-arg=-Wl,-rpath,...` in a build script.
For dev runs, `DYLD_FALLBACK_LIBRARY_PATH` is enough. SIP strips `DYLD_*` variables for system
binaries, not for ours.

## Ubuntu 22.04/24.04 x64 (expected; run in WP-0E.03)

```sh
sudo apt install clang libclang-dev pkg-config python3
just ffmpeg-fetch                 # or distro libav*-dev for hacking only (Ubuntu 24.04 ships FFmpeg 6.1)
. ./env.sh                        # FFMPEG_DIR + LD_LIBRARY_PATH=$FFMPEG_DIR/lib
cargo build --release && ./target/release/s3 all
```

The bundle's `.so` files use `$ORIGIN` rpaths among themselves. The product binary needs
`-Wl,-rpath,$ORIGIN/../lib` (or wherever the packaging puts them). **Distro FFmpeg 6.1 also
works with ffmpeg-next 9.1** (it detects `ffmpeg_6_1`), but its results do not count toward S3,
which measures the pinned 8.1.
