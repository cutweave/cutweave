# M1 — Phase 1 exit (walking skeleton)

> **Status: ready for owner sign-off.** Drafted by the WP-1O.04 agent on 2026-11-25 from a `just m1-gate` run (evidence below). The go/no-go call is the product owner's.
> Per **R34** ([05-work-breakdown.md](../05-work-breakdown.md)), M1 is verified **on Windows only** for now: one Windows 11 machine (`ref-win-dgpu`, [reference-machines.md](../reference-machines.md)). **macOS and Linux are deferred until those machines exist, not dropped.**

The exit criteria are those of [P1-skeleton.md](../wbs/P1-skeleton.md#exit-criteria-m1-copied-from-roadmap); the suite is WP-1O.04 (GitHub issue #105).

## How to reproduce

```sh
just ffmpeg-fetch   # once: the FFmpeg dev bundle (docs/dev/ffmpeg.md)
just corpus         # once: the test corpus (never committed)
just m1-gate --full --evidence docs/milestones/m1-evidence
```

`just m1-gate` ([scripts/m1-gate.py](../../scripts/m1-gate.py)) builds the engine and the CLI with FFmpeg and the desktop app, runs the suite below and prints pass or fail per criterion. Its report, logs and media go to `target/m1/` (not committed); `--evidence` writes the text evidence (no media) to [m1-evidence/](m1-evidence/). `--trials N` sets the crash trials of each kind (default 20), `--full` adds `just check` and `just ui-snapshots` (criterion 4a), `--criteria 1,2` runs a subset. `python scripts/m1-gate.py compare UI_DIR MCP_DIR` repeats criterion 2's comparison on two kept runs.

## Result

EVIDENCE-RESULT-TABLE

## 1. Through the UI: import 3 clips, arrange, trim, split, export a 1080p MP4 that plays

| What ran | Result |
|---|---|
| [`e2e/m1/m1-exit.spec.ts`](../../apps/desktop/e2e/m1/m1-exit.spec.ts): the three M1 corpus clips (H.264 1080p60 with sound, HEVC 10-bit without, VFR phone clip rotated 90° with sound) imported with the bin's Import button; each placed with Enter at the playhead and trimmed by typing its Duration in the inspector (A 0–4 s, B 4–7 s, C 7–10 s on V1); A split at 2 s with S; A's right half moved to 10 s by typing its Start; Ctrl+S; File export from the top bar (1080p preset at High quality, software encoder) | EVIDENCE-UI |
| The export checked by the gate: decoded to the end, sound level, and frames 30, 150 and 255 decoded from it against the same frames rendered by `cutweave render` from the saved project. The bound (tolerance policy: PSNR ≥ 38 dB for a lossy MP4 against a rendered frame) is measured in the export's own colour space, BT.709 limited-range 4:2:0, which is the encoder's loss; the RGB round trip is reported beside it (finding 3) | EVIDENCE-VALIDATE |
| The user flow F14 ([`f14-export.spec.ts`](../../apps/desktop/e2e/flows/f14-export.spec.ts)): three other corpus clips placed with pointer drags, trimmed with a drag and the inspector, split, the export dialog's settings, cancel, export, replace prompt | EVIDENCE-F14 |

Evidence: [ui-session.json](m1-evidence/ui-session.json) (layout, ffprobe, the Chromium `<video>` readout), [validate-mp4.json](m1-evidence/validate-mp4.json).

## 2. Through MCP headless: a scripted client does the same and exports an identical-timeline file

| What ran | Result |
|---|---|
| The MCP conformance suite with the M1 scripted agent ([`mcp_conformance/m1.rs`](../../crates/cutweave-cli/tests/mcp_conformance/m1.rs)), `cutweave` built with FFmpeg: `cutweave mcp --headless --deterministic-ids 42` on the full engine; MCP tools only (`media_import`, `job_wait`, `timeline_insert`, `clip_trim`, `clip_split`, `item_move`, `timeline_get`, `render_frame`, `export_start`, `cutweave_execute`); every result checked against its command's output schema; the export checked with ffprobe | EVIDENCE-MCP |
| The UI's and the agent's saved projects compared by normalized structure, their exports by streams, length and decoded frames | EVIDENCE-COMPARE |

**Normalization** (`normalize_project` in the gate, self-tested by `python scripts/m1-gate.py --self-test`): every entity id is replaced by a name derived from the structure (media by file name, tracks by their place in the stack, items by track and start, linked groups by first appearance), media paths become file names, and `meta.created`, `meta.modified` and `meta.app_version` are dropped. Nothing else is: probe data, content hashes, formats, every item field. Any id the normalizer does not know stays and fails the comparison.

**Exports**: equal stream descriptions (codec, profile, size, pixel format, frame rate, colour, sample rate, channels, frame count), lengths within one frame, and decoded frames equal at every 10th frame (else PSNR ≥ 45 dB, WP-1O.04).

Evidence: [compare.json](m1-evidence/compare.json) (with the normalized timeline), [mcp-session.json](m1-evidence/mcp-session.json).

## 3. Kill the engine mid-edit: the UI recovers with no lost commands

| What ran | Result |
|---|---|
| [`e2e/m1/m1-crash.spec.ts`](../../apps/desktop/e2e/m1/m1-crash.spec.ts), engine kills: a 30-command script (inserts, delta trims, splits, moves, renames, new tracks) through the editor's own engine connection; the engine SIGKILLed after command k, k spread over 1–30, with command k in flight in odd trials and answered in even ones, and the next two commands sent while it is down; the document and the undo history compared with an uninterrupted run. One in five trials kills it mid-drag instead (a gesture transaction): the drag must roll back to the last committed state and land once when repeated | EVIDENCE-ENGINE-KILL |
| The same file, whole-app kills: after k answered commands (k from 3 to 30), the app and the engine killed at once, relaunched, the project opened from Recent, Restore in the recovery dialog; the document compared with the last acknowledged state, ids included; Ctrl+S writes it to disk | EVIDENCE-APP-KILL |
| The user flows F11 (engine killed mid-edit, a split typed while it is down) and F12 (app killed, Restore and Discard) | EVIDENCE-F11-F12 |

Evidence: [crash-engine-kill.json](m1-evidence/crash-engine-kill.json), [crash-app-kill.json](m1-evidence/crash-app-kill.json).

## 4. CI green on 3 OSes; golden frames pass; the parity test passes

| Item | What ran | Result |
|---|---|---|
| 4a | R34: on Windows with local gates (Actions-minutes policy, owner 2026-11-25): the full local gate `just check`, run by `just check-affected` on this branch (it falls back to `just check` because the justfile changed) together with the user-flow suite. No CI workflow was added or run: no CI run URLs | EVIDENCE-4A |
| 4b | `cargo xtask golden` on the software adapter (WARP): six projects, 18 frames, SSIM ≥ 0.995 and PSNR ≥ 40 dB against `expected/dx12-warp`, each frame also bit-identical on a second render and through the export readback | EVIDENCE-4B |
| 4c | `cargo nextest run -p cutweave-api --test parity`: CLI = MCP = RPC, byte-identical saved documents | EVIDENCE-4C |

## Deferred (not dropped)

- **macOS and Linux** (R34): the suite, the golden references (`vulkan-llvmpipe`, `metal-<gpu>`) and "CI green on 3 OSes". They run when those machines exist; `just m1-gate` itself is not Windows-specific.
- **CI aggregation and run URLs**: WP-1O.04 asks for an `m1-gate` workflow. None was added (Actions-minutes policy): the local `just m1-gate` aggregates the same checks. When the owner wants CI evidence, a manual-dispatch workflow running `python scripts/m1-gate.py` on `windows-latest` is the whole of it.
- **The demo video** of the flow, one per OS (WP-1O.04 gives it to a person).

## Found by the suite

EVIDENCE-FINDINGS

## Where this differs from the WP text, and why

- **Ids:** only the agent's engine runs with `--deterministic-ids 42`. A desktop (GUI) engine refuses the flag unless it is also told `--i-know-this-is-a-test`, which would be a test-only switch in the app; the comparison replaces ids anyway.
- **Normalizer and MP4 checks** live in the gate script (Python, like `check-affected.py`), not in `xtask normalize-project` / `xtask validate-mp4`: they read JSON and call ffprobe and ffmpeg, so nothing needs compiling.
- **Same encoder on both sides:** the UI exports with the software encoder at High quality, which is what the agent's default request (`h264-mp4-1080p`, `hw: "off"`) means. Hardware encoders do not promise identical output, so the frame comparison would fall back to PSNR.
- **Playhead to the end:** "Go to end" stops on the last frame, not after it, so Enter there would overlap the clip; a click on the ruler past the end puts the playhead at the end (the engine stops it there), checked on the timecode.
- **Crash trial commands** go through the editor's own connection (the outbox, resend, journal replay and resync are the real ones), as the WP-1L.03 tests do; the pointer is used for the drag trials. F11 and F12 cover the same journeys with clicks. Between engine kills the supervisor's crash-loop guard (four crashes in 60 s) is reset with the existing test hook: it is not what is measured.
- **Report path:** this file, next to [M0](M0-go-no-go.md), instead of `docs/m1-acceptance.md`.

## Open questions for the owner

1. **Windows-only sign-off (R34):** is M1 accepted on this Windows evidence, with "CI green on 3 OSes" deferred until macOS and Linux machines exist?
2. **CI run URLs:** do the local gates stand in for CI run URLs at M1, or should a manual-dispatch `m1-gate` workflow be added and run once (Actions minutes)?
3. **Demo video:** who records the Windows demo of the flow (the WP gives it to a person)?
4. The **findings** above: which go into Phase 2 work packages, and is the MCP schema fix (finding 1) a contract change that needs an ADR?

## Product-owner sign-off

☐ GO ☐ GO with changes ☐ NO-GO (date, notes)
