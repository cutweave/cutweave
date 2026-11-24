# testdata/

Media corpus manifest (the corpus itself is fetched by `just corpus`, never committed), golden frames (`testdata/golden/`, stored in LFS) and the command scripts of the CLI = MCP = RPC parity test ([`scripts/`](scripts/README.md), WP-1O.02).

## Corpus (WP-0I.01)

- `corpus.toml`: the manifest. Each item records id, category, description, purpose, codec, container, license (CC0-1.0, CC-BY-3.0/4.0, public-domain or self-generated; `cargo xtask` rejects an entry without one), attribution (required for CC-BY), source URL or generator recipe, sha256, size and tier.
- `generate/*.sh`: ffmpeg `lavfi` recipes for the 16 self-generated items (VFR, rotated, HDR10, ProRes, odd sample rates, 5.1, broken and variable-size files). Project-generated, so licensing is clean. `vfr_phone_rotated90` is two passes (VFR encode, then `-display_rotation` as an input option with `-c copy`); `variable_frame_size` joins three MPEG-TS segments with the `concat:` protocol from beside the output. They need FFmpeg on `PATH` and are skipped (not failed) without it. Pinned hashes are only meaningful for the FFmpeg version recorded in `[meta]`.
- `corpus/`: the git-ignored download/generation cache. Tests find files by manifest id.

```
cargo xtask fetch-corpus [--tier small|full]   # small (< 50 MB, PR CI) is the default; full = nightly
cargo xtask fetch-corpus --pin                 # first fetch: record sha256 and size in the manifest
cargo xtask corpus-verify [--tier ..]          # re-hash the cache; fails on missing, unpinned or corrupt files, and probes generated items
cargo xtask fetch-corpus --verify              # fetch, then run the same verification (unpinned items allowed)
```

`just corpus` runs `fetch-corpus` and then `corpus-verify --allow-unpinned`, so a recipe that produces the wrong media fails the recipe step of CI and local setup.

**Which bash runs the recipes.** `fetch-corpus` calls the recipes with forward-slash paths. On Windows it prefers Git Bash (the `bin\bash.exe` next to the `git` on `PATH`, then the standard Git for Windows folders) and never uses the WSL launcher in `System32` or `WindowsApps`, which has its own file system and ffmpeg. Set `CUTWEAVE_BASH` to a bash path to override. Recipes use plain bash and ffmpeg 6 or later options, and keep their scratch files beside the output (never `/tmp`), so they run unchanged on Linux and macOS.

### Verifying generated media (`expect`)

Each generated item carries an `expect` inline table in `corpus.toml`. `corpus-verify` runs `ffprobe` on the file and compares only the fields given:

| Field | Checked against |
|---|---|
| `codec` (the item's own field) | `codec_name` of any stream |
| `width`, `height`, `fps` | first video stream (`fps` is `r_frame_rate`, 0.1 % tolerance) |
| `vfr` | `true` when video packet presentation times are unevenly spaced (more than 2 ms apart between the shortest and longest gap); `false` for constant frame rate |
| `rotation` | display-matrix rotation in degrees **clockwise** (the engine's convention; ffprobe prints counter-clockwise, so `-display_rotation 90` is `270` here); absent means 0 |
| `pix_fmt`, `color_transfer` | first video stream |
| `frame_sizes` | the distinct decoded frame sizes, in order of first appearance (variable frame size) |
| `audio`, `channels`, `sample_rate` | presence and layout of the first audio stream |
| `duration`, `duration_tolerance` | container duration in seconds (default tolerance 0.25 s) |
| `broken` | the file must fail to probe (the truncated MP4) |

Without `ffprobe` on `PATH` the probe step is skipped with a message; `--probe` turns that into an error and `--no-probe` skips the step. Add an `expect` entry when you add a generated item.

Downloads use `curl` (resumable with `-C -`), go to `<file>.part` and are renamed only after the sha256 matches; a mismatch deletes the download. Items with `sha256 = "PENDING"` are pinned on the first fetch by a maintainer (`--pin`) and the manifest change is committed. `url_verified = false` marks URLs written from the upstream project's documented layout that have not been fetched yet.

**Hosting of any file too large or unstable to fetch from upstream:** GitHub Releases assets of a `cutweave-testdata` repository (not created yet).

### Attribution and license audit of fetched items

Only CC0, CC-BY, public-domain and project-generated media may enter the corpus (docs/01-architecture.md §21). Items marked **review required** must have the exact file and its license confirmed before use; until then they are excluded from any release artifact.

| Item | License | Attribution | Audit |
|---|---|---|---|
| `sintel_trailer_480p` | CC-BY-3.0 | (c) copyright Blender Foundation | durian.blender.org (Sintel), CC BY 3.0 | ok |
| `bbb_1080p_h264` | CC-BY-3.0 | (c) copyright 2008, Blender Foundation | www.bigbuckbunny.org, CC BY 3.0 | ok |
| `sintel_4k_mkv` | CC-BY-3.0 | (c) copyright Blender Foundation | durian.blender.org (Sintel), CC BY 3.0 | ok |
| `tears_of_steel_720p` | CC-BY-3.0 | (CC) Blender Foundation | mango.blender.org (Tears of Steel), CC BY 3.0 | ok |
| `wikimedia_pd_jpeg` | public-domain | none required | **review required** |
| `bbb_4k_webm_wikimedia` | CC-BY-3.0 | (c) copyright 2008, Blender Foundation | www.bigbuckbunny.org, CC BY 3.0 | **review required** |
