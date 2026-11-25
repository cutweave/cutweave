# M1 — Phase 1 exit (walking skeleton)

> **Status: ready for owner sign-off.** Drafted by the WP-1O.04 agent on 2026-11-25 from a `just m1-gate` run on commit `1744a14` (main `8d844e8` merged; the only uncommitted change was this report's text), evidence below. The go/no-go call is the product owner's.
> Per **R34** ([05-work-breakdown.md](../05-work-breakdown.md)), M1 is verified **on Windows only** for now: one Windows 11 machine (`ref-win-dgpu`, [reference-machines.md](../reference-machines.md)). **macOS and Linux are deferred until those machines exist, not dropped.**

The exit criteria are those of [P1-skeleton.md](../wbs/P1-skeleton.md#exit-criteria-m1-copied-from-roadmap); the suite is WP-1O.04 (GitHub issue #105).

## How to reproduce

```sh
just ffmpeg-fetch   # once: the FFmpeg dev bundle (docs/dev/ffmpeg.md)
just corpus         # once: the test corpus (never committed)
just m1-gate --evidence docs/milestones/m1-evidence
```

`just m1-gate` ([scripts/m1-gate.py](../../scripts/m1-gate.py)) builds the engine and the CLI with FFmpeg and the desktop app, runs the suite below and prints pass or fail per criterion. Its report, logs and media go to `target/m1/` (not committed); `--evidence` writes the text evidence (no media) to [m1-evidence/](m1-evidence/). `--trials N` sets the crash trials of each kind (default 20), `--full` adds `just check` and `just ui-snapshots` (criterion 4a), `--criteria 1,2` runs a subset. `python scripts/m1-gate.py compare UI_DIR MCP_DIR` repeats criterion 2's comparison on two kept runs.

## Result

| Exit criterion | Result on Windows |
|---|---|
| 1. Through the UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays | **pass** |
| 2. Through MCP headless: a scripted client does the same and exports an identical-timeline file | **pass**: the same timeline, and a byte-identical MP4 |
| 3. Kill the engine mid-edit: the UI recovers with no lost commands | **pass**: 0 lost and 0 doubled commands in 40 trials |
| 4. CI green on 3 OSes; golden frames pass; parity passes | **pass on Windows** (R34): local full gate, 18/18 golden frames bit-identical, parity 8/8. macOS, Linux and CI run URLs deferred |

The run: [report.md](m1-evidence/report.md) (every step with its time and result; [report.json](m1-evidence/report.json)), on an AMD Ryzen laptop with an NVIDIA RTX 3080 Laptop GPU and AMD Radeon graphics, FFmpeg n8.1.3, Node 24.19, rustc 1.100. The end of each step's log is in [m1-evidence/logs/](m1-evidence/logs/).

## 1. Through the UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays

| What ran | Result |
|---|---|
| [`e2e/m1/m1-exit.spec.ts`](../../apps/desktop/e2e/m1/m1-exit.spec.ts): the three M1 corpus clips (H.264 1080p60 with sound, HEVC 10-bit without, VFR phone clip rotated 90° with sound) imported with the bin's Import button; each placed with Enter at the playhead and trimmed by typing its Duration in the inspector (A 0–4 s, B 4–7 s, C 7–10 s on V1); A split at 2 s with S; A's right half moved to 10 s by typing its Start; Ctrl+S; File export from the top bar (1080p preset at High quality, software encoder) | **pass** (14 s): the timeline V1 0–2, 4–7, 7–10, 10–12 s and A1 0–2, 7–10, 10–12 s; `M1 cut.mp4` is H.264 1920×1080 at 30 fps with AAC, 12.000 s (the timeline exactly); Chromium plays it (`readyState` 4, 1920×1080, 12 s, no error) |
| The export checked by the gate: decoded to the end, sound level, and frames 30, 150 and 255 decoded from it against the same frames rendered by `cutweave render` from the saved project. The bound (tolerance policy: PSNR ≥ 38 dB for a lossy MP4 against a rendered frame) is measured in the export's own colour space, BT.709 limited-range 4:2:0, which is the encoder's loss; the RGB round trip is reported beside it (finding 3) | **pass**: decodes to the end with no error; loudest sample −14.8 dB; frames 30, 150 and 255: 48.9, 48.9 and 47.7 dB (RGB round trip 37.5, 37.3 and 35.3 dB) |
| The user flow F14 ([`f14-export.spec.ts`](../../apps/desktop/e2e/flows/f14-export.spec.ts)): three other corpus clips placed with pointer drags, trimmed with a drag and the inspector, split, the export dialog's settings, cancel, export, replace prompt | **pass** (13 s) |

Evidence: [ui-session.json](m1-evidence/ui-session.json) (layout, ffprobe, the Chromium `<video>` readout), [validate-mp4.json](m1-evidence/validate-mp4.json).

## 2. Through MCP headless: a scripted client does the same and exports an identical-timeline file

| What ran | Result |
|---|---|
| The MCP conformance suite with the M1 scripted agent ([`mcp_conformance/m1.rs`](../../crates/cutweave-cli/tests/mcp_conformance/m1.rs)), `cutweave` built with FFmpeg: `cutweave mcp --headless --deterministic-ids 42` on the full engine; MCP tools only (`media_import`, `job_wait`, `timeline_insert`, `clip_trim`, `clip_split`, `item_move`, `timeline_get`, `render_frame`, `export_start`, `cutweave_execute`); every result checked against its command's output schema; the export checked with ffprobe | **pass**: 7 of 7 tests (four conformance runs, the broken-schema negative test, the stub scripted agent, the M1 agent); the agent's export job `succeeded`: 1920×1080, 30 fps, 360 frames |
| The UI's and the agent's saved projects compared by normalized structure, their exports by streams, length and decoded frames | **pass**: normalized timelines equal (7 clips); stream descriptions equal; both 12.000 s; the 36 decoded frames checked (every 10th) identical, the decoded sound identical, and the two files **byte-identical** (5,907,413 bytes) |

**Normalization** (`normalize_project` in the gate, self-tested by `python scripts/m1-gate.py --self-test`): every entity id is replaced by a name derived from the structure (media by file name, tracks by their place in the stack, items by track and start, linked groups by first appearance), media paths become file names, and `meta.created`, `meta.modified` and `meta.app_version` are dropped. Nothing else is: probe data, content hashes, formats, every item field. Any id the normalizer does not know stays and fails the comparison.

**Exports**: equal stream descriptions (codec, profile, size, pixel format, frame rate, colour, sample rate, channels, frame count), lengths within one frame, and decoded frames equal at every 10th frame (else PSNR ≥ 45 dB, WP-1O.04).

Evidence: [compare.json](m1-evidence/compare.json) (with the normalized timeline), [mcp-session.json](m1-evidence/mcp-session.json).

## 3. Kill the engine mid-edit: the UI recovers with no lost commands

| What ran | Result |
|---|---|
| [`e2e/m1/m1-crash.spec.ts`](../../apps/desktop/e2e/m1/m1-crash.spec.ts), engine kills: a 30-command script (inserts, delta trims, splits, moves, renames, new tracks) through the editor's own engine connection; the engine SIGKILLed after command k, k spread over 1–30, with command k in flight in odd trials and answered in even ones, and the next two commands sent while it is down; the document and the undo history compared with an uninterrupted run. One in five trials kills it mid-drag instead (a gesture transaction): the drag must roll back to the last committed state and land once when repeated | **pass**, 20 trials: 16 script trials killed after command 1, 3, 5, 7, 9, 11, 13, 15, 16, 18, 20, 22, 24, 26, 28 and 30 (8 with the command in flight), 30 commands sent during the outages: every document and undo history equal to the uninterrupted run's. 4 drag trials: each drag rolled back ("Clip not moved"), the repeated drag landed once (one undo entry). Back online 0.9–1.9 s after the kill (median 0.9 s) |
| The same file, whole-app kills: after k answered commands (k from 3 to 30), the app and the engine killed at once, relaunched, the project opened from Recent, Restore in the recovery dialog; the document compared with the last acknowledged state, ids included; Ctrl+S writes it to disk | **pass**, 20 trials, k = 5–30 (20 different values): the dialog offered k + 1 edits (the import and the k commands) every time; every restored document equal to the last acknowledged one, ids included; every save on disk. The unsaved dot stayed on after the save in 20 of 20 trials (finding 2) |
| The user flows F11 (engine killed mid-edit, a split typed while it is down) and F12 (app killed, Restore and Discard) | **pass** (F11 3 s, F12 4 s) |

Evidence: [crash-engine-kill.json](m1-evidence/crash-engine-kill.json), [crash-app-kill.json](m1-evidence/crash-app-kill.json).

## 4. CI green on 3 OSes; golden frames pass; the parity test passes

| Item | What ran | Result |
|---|---|---|
| 4a | R34: on Windows with local gates (Actions-minutes policy, owner 2026-11-25): the full local gate `just check`, run by `just check-affected` on this branch (it falls back to `just check` because the justfile changed) together with the user-flow suite. No CI workflow was added or run: no CI run URLs | **pass**: `just check-affected --base 8d844e8` exit 0 on this branch (it ran the full `just check`, the user-flow suite and the golden frames) |
| 4b | `cargo xtask golden` on the software adapter (WARP): six projects, 18 frames, SSIM ≥ 0.995 and PSNR ≥ 40 dB against `expected/dx12-warp`, each frame also bit-identical on a second render and through the export readback | **pass**: `dx12-warp`, 18 frames, 18 bit-identical with the references, 0 within tolerance, 0 failed; the 1-pixel negative test fails as it must |
| 4c | `cargo nextest run -p cutweave-api --test parity`: CLI = MCP = RPC, byte-identical saved documents | **pass**: 8 of 8 tests (every script byte-identical on CLI, MCP and RPC; every registry entry reachable with the registry schema) |

## Deferred (not dropped)

- **macOS and Linux** (R34): the suite, the golden references (`vulkan-llvmpipe`, `metal-<gpu>`) and "CI green on 3 OSes". They run when those machines exist; `just m1-gate` itself is not Windows-specific.
- **CI aggregation and run URLs**: WP-1O.04 asks for an `m1-gate` workflow. None was added (Actions-minutes policy): the local `just m1-gate` aggregates the same checks. When the owner wants CI evidence, a manual-dispatch workflow running `python scripts/m1-gate.py` on `windows-latest` is the whole of it.
- **The demo video** of the flow, one per OS (WP-1O.04 gives it to a person).

## Found by the suite

None of these loses an edit or breaks a criterion; each is for a Phase 2 work package.

1. **MCP output schemas do not describe MCP results.** `cutweave_describe_command` serves the registry's (RPC) output schema, but MCP results differ from it in two documented ways: text that comes from media files is wrapped as `{"untrusted": true, "text": ...}` (`results::UNTRUSTED_FIELDS`: `job_wait`, `job_status`, `job_list` and `job_cancel` progress messages and errors, `media_import` failures), and `render_frame` moves the picture into an image block (no `data`). A client that validates `structuredContent` against the described schema rejects, for example, every `job_wait` on a real analysis job. Real media showed it first: the stub engine queues no jobs. The conformance checker had worked around it ([main.rs](../../crates/cutweave-cli/tests/mcp_conformance/main.rs), `unwrap_untrusted`, now removed); fixed by issue #146: `cutweave_describe_command` now describes the MCP shape and the checker no longer unwraps (ADR-0028, Proposed).
2. **After a crash Restore, saving leaves the unsaved dot on.** In 20 of 20 whole-app trials of the recorded run (15 of 20 in an earlier one), Ctrl+S after Restore writes the restored edits to the project file (checked on disk every time) but the title bar keeps "Unsaved changes" and main keeps the window dirty, so quitting asks to save again. The editor follows the engine's `project.dirty` event only (`session/sessionSync.ts`), so the likely cause is that event not being sent after a save that follows `project.recover` (`crates/cutweave-engine/src/autosave`). The trials record it per trial as `dirtyAfterSave` ([crash-app-kill.json](m1-evidence/crash-app-kill.json)) without failing on it.
3. **The tolerance policy does not say in which colour space a lossy MP4 is compared with a rendered frame.** Measured in the export's BT.709 limited-range 4:2:0 space, as the encoder sees it, frames are 47.7–48.9 dB from `cutweave render`. Converted back to RGB they are 35.3–37.5 dB on these saturated synthetic clips: 4:2:0 chroma subsampling alone costs that in red and blue at the colour-bar edges. The gate applies the 38 dB bound to the first and reports the second (`crates/cutweave-engine/tests/export_ffmpeg.rs` measures RGB on its own, less saturated content).
4. **Relaunching right after a forced kill can lose to Chromium's single-instance lock.** The killed app's last helper process holds `<userData>/lockfile` for a moment, and a relaunch on the same profile then exits at once. The app-kill trials wait for the lock to be free; F12 relaunches the same way, twice, and could flake the same way.
5. **Engine readiness (#143) was not hit**: every launch of the M1 suite reached "Engine connected".
6. The notice after a drag cut short by an engine crash says "Clip not moved: The project changed while you were editing. Try again." The outcome is right (rolled back, nothing lost); the reason it gives is not the real one.

## Where this differs from the WP text, and why

- **Ids:** only the agent's engine runs with `--deterministic-ids 42`. A desktop (GUI) engine refuses the flag unless it is also told `--i-know-this-is-a-test`, which would be a test-only switch in the app; the comparison replaces ids anyway.
- **Normalizer and MP4 checks** live in the gate script (Python, like `check-affected.py`), not in `xtask normalize-project` / `xtask validate-mp4`: they read JSON and call ffprobe and ffmpeg, so nothing needs compiling.
- **Same encoder on both sides:** the UI exports with the software encoder at High quality, which is what the agent's default request (`h264-mp4-1080p`, `hw: "off"`) means. Hardware encoders do not promise identical output, so the frame comparison would fall back to PSNR.
- **The playhead is placed with ruler clicks**, each checked on the timecode to the frame: at 2 s for the split, and past the end of the timeline before each Enter, because "Go to end" stops on the last frame (Enter there would overlap the clip) while a seek past the end is stopped by the engine exactly at the end. No drag is used for the cut.
- **Crash trial commands** go through the editor's own connection (the outbox, resend, journal replay and resync are the real ones), as the WP-1L.03 tests do; the pointer is used for the drag trials. F11 and F12 cover the same journeys with clicks. Between engine kills the supervisor's crash-loop guard (four crashes in 60 s) is reset with the existing test hook: it is not what is measured.
- **Report path:** this file, next to [M0](M0-go-no-go.md), instead of `docs/m1-acceptance.md`.

## Open questions for the owner

1. **Windows-only sign-off (R34):** is M1 accepted on this Windows evidence, with "CI green on 3 OSes" deferred until macOS and Linux machines exist?
2. **CI run URLs:** do the local gates stand in for CI run URLs at M1, or should a manual-dispatch `m1-gate` workflow be added and run once (Actions minutes)?
3. **Demo video:** who records the Windows demo of the flow (the WP gives it to a person)?
4. The **findings** above: which go into Phase 2 work packages, and is the MCP schema fix (finding 1) a contract change that needs an ADR?

## Product-owner sign-off

☐ GO ☐ GO with changes ☐ NO-GO (date, notes)
