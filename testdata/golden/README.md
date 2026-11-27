# Golden frames

Reference frames for the render pipeline, stored per GPU adapter class and compared with the single-sourced tolerance policy ([docs/wbs/P1-skeleton.md](../../docs/wbs/P1-skeleton.md#tolerance-policy-single-source-for-this-phase), [arch §20](../../docs/01-architecture.md#20-testing-strategy)). Two suites live here:

| Folder | Suite | Run |
|---|---|---|
| `expected/<adapter>/<project>/f<frame>.png` | **Golden projects** (WP-1O.01, WP-2C.04, WP-2C.08, WP-2B.02): thirteen `.cutweave` projects rendered headless by `cutweave render`, 39 frames | `just golden` |
| `render/<adapter>/*.png` | Compositor scenes of WP-1F.02 and WP-2C.04 (`cutweave-render` tests; `CUTWEAVE_UPDATE_GOLDENS=1` rewrites the sets of every adapter present) | `cargo nextest run -p cutweave-render` |

The PNGs are in Git LFS. Worktrees and LFS-less checkouts hold pointer files; `cargo xtask golden` runs `git lfs checkout` on the set it needs, and `git lfs pull --include "testdata/golden/**"` fetches objects that are not in the local store.

## Adapter classes

A set is named by `AdapterInfo::key`, `<backend>-<adapter>`: by adapter, never by machine. Software rasterisers are named by family because their names carry versions.

| Set | Adapter | Status |
|---|---|---|
| `dx12-warp` | WARP (Windows software rasteriser) | **CI class** (`windows-latest`, the `golden-frames` job) and every Windows machine |
| `dx12-nvidia-geforce-rtx-3080-laptop-gpu` | NVIDIA RTX 3080 Laptop, DX12 | local class (the reference Windows laptop) |
| `dx12-amd-radeon-tm-graphics` | AMD Radeon iGPU, DX12 | local class (the same laptop) |
| `vulkan-llvmpipe` | lavapipe (Linux software rasteriser) | deferred: no Linux machine yet (R34) |
| `metal-<gpu>` | Metal on Apple silicon | deferred: no Mac yet (R34) |

M1 is verified on Windows only for now (R34 in [05-work-breakdown.md](../../docs/05-work-breakdown.md)). A new class needs no code: run the update below on that machine and commit the new folder.

A frame is only ever compared with the reference of **the adapter it rendered on**. There is no fallback to another adapter's set: a missing set fails with the command that creates it.

## The projects

`cargo xtask golden-projects` builds them into `target/golden/` (never committed, rebuilt on every run because a saved project records the absolute path and content hash of its media):

- **Sources:** FFmpeg `lavfi` test patterns (`smptehdbars`, `smptebars`, `testsrc2`), 640×360, 30 fps, 4 s, H.264 lossless 4:2:0 with explicit colour tags, and one still (`rgbtestsrc`, 480×480, a PNG) for the image background, generated into `target/golden/sources/` (never into `testdata/corpus`). Project-generated, so there is no licensing question.
- **Projects:** built by one `cutweave mcp --headless` engine through `cutweave_execute` with real commands (`project.new`, `media.import` with the FFmpeg probe, `timeline.insert`, `track.add`, `item.set_props`, `keyframe.set`, `clip.split`, `item.delete`, `project.save_as`), seeded ids. The recipes are in [`xtask/src/golden/projects.rs`](../../xtask/src/golden/projects.rs).

| Project | What it covers | Frames |
|---|---|---|
| `bars_100` | one colour-bars clip at 100% | 0, 60, 119 |
| `scaled_offset` | a moving pattern at 50%, offset from the centre | 0, 45, 110 |
| `opacity_over_track` | 50% opacity on V2 over bars on V1 | 0, 45, 110 |
| `rotated_phone` | a 90° display-matrix rotation (portrait in a landscape canvas) | 0, 45, 110 |
| `color_tags` | BT.601 limited range beside BT.709 full range, same samples | 0, 45, 110 |
| `cut_at_45` | a split at frame 45, the right half replaced: frames 44 and 45 straddle the cut | 20, 44, 45 |
| `flipped_phone` | the rotated phone clip cropped, flipped and turned another 90° by its transform (WP-2C.04) | 0, 45, 110 |
| `screen_keyframed` | a half-size pattern in screen mode over bars, its position and rotation keyframed (WP-2C.04) | 0, 45, 110 |
| `side_by_side` | two colour-bars clips arranged by `item.apply_layout` (side by side, fill): each half filled, no bars (WP-2C.08) | 0, 60, 119 |
| `background_color` | the 16:9 moving pattern on a 9:16 sequence (360×640) over a plain colour (WP-2B.02) | 0, 45, 110 |
| `background_blur` | the same over a blurred, dimmed copy of the picture (`sequence.set_background`, kind `blur`); the frame is dithered (WP-2B.02) | 0, 45, 110 |
| `background_image` | the same over a square still image covering the canvas (kind `image`, fit `cover`) (WP-2B.02) | 0, 45, 110 |
| `background_transparent` | the same over nothing (kind `transparent`): alpha 0 above and below the clip (WP-2B.02) | 0, 45, 110 |

## What a run checks

`cargo xtask golden` (`just golden` sources the FFmpeg environment first):

1. **Update guard** (below).
2. Builds `cutweave` with FFmpeg, the sources and the projects.
3. Renders each project with `cutweave render --frames ... --out-dir ... --check-determinism` and `CUTWEAVE_GPU=software` (default; `--adapter <choice>` picks another, `--all-adapters` runs every local adapter that has a set). Each frame is rendered by `render.frame`, a second time, and through the export pipeline's readback on a separate compositor: the three must be **bit-identical**, or the render fails.
4. Compares each frame with `expected/<adapter>/`: it passes when it is bit-identical, or when **SSIM ≥ 0.995 and PSNR ≥ 40 dB** (`cutweave_render::metrics`, the only place the numbers live). A failure writes `target/golden/failures/<adapter>/<project>/f<frame>.{actual,expected,diff}.png` (CI uploads them).
5. **Negative test:** `scaled_offset` moved one pixel to the right must fail against `scaled_offset`'s references, every frame; its evidence goes to `target/golden/negative/<adapter>/`.

`target/golden/report.json` holds every verdict and metric.

## Updating references

References change only on purpose, for one adapter at a time, on a machine that has that adapter:

```
just golden --update --i-know --reason "why the pixels changed"                       # WARP
just golden --update --i-know --adapter key:dx12-nvidia-geforce-rtx-3080-laptop-gpu   # another class
```

This rewrites the frames whose pixels changed (unchanged ones keep their bytes), removes stale frames of that set, and appends an entry to [UPDATES.md](UPDATES.md) naming the adapter and the set's **digest** (16 hex digits of a SHA-256 over every file's path and content hash). Commit the PNGs and the entry together, then give the pull request the `golden-update` label. Edit the entry's reason if it says none; keep the digest.

**The guard** (`cargo xtask golden --check-refs [--base REV] [--pr-labels A,B]`, also the first step of every run): every adapter set with a reference added, changed or removed since the base commit (the merge base with `main`, as in `just check-affected`) needs an entry added to `UPDATES.md` in the same change that names the set and carries its current digest. A PNG edited, added or removed by hand, even after a proper update, changes the digest and fails the guard; rerunning the update records the new state. In CI the `golden-frames` job also requires the `golden-update` label. The guard also enforces the size budget: each PNG under 150 KB, the whole `expected/` under 10 MB.
