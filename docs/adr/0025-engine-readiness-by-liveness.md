# ADR-0025: The ready line means serving; the supervisor judges a start by liveness

- **Status:** Accepted (product owner, 2026-11-26)
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (drafted by the issue #143 fix)
- **Supersedes:** none (amends the supervision rules of the [launch protocol](../reference/launch-protocol.md), WP-1L.03, and the GPU wait of WP-1I.05)
- **Architecture refs:** `docs/01-architecture.md` §2, §8, §19

## Context

Electron main's `EngineSupervisor` gave the engine 10 s from the spawn to its ready line. A missed ready line counted as a crash, and four crashes in 60 s gave `failed`: the "The engine stopped 4 times in 60 s" dialog the product owner saw on the first day of real use (issue #143). On a busy machine the full engine missed the 10 s often.

Measured on the dev laptop (Windows, debug build, 16 CPU hogs on 16 threads, six starts each; per-phase timings now in the engine log):

| | Ready after spawn | Where the time went |
|---|---|---|
| Before | 6.8, 11.0, 6.9, 10.7, 12.4, 16.0 s (median 11 s) | process load 0.6–1.8 s; caches, then the audio output (1.0–5.2 s: it waits for the device's first callback), then the full 4 s GPU wait, which started only after them; then the server, 0.5–5.2 s, almost all of it the run-file's `whoami` and `icacls` |
| After | 5.1, 4.3, 4.4, 4.6, 4.9, 4.6 s (median 4.6 s) | process load 0.2–1.0 s; the 4 s GPU window, with the caches, audio and ring inside it; the server 5–27 ms |
| Before, rerun later the same day | 9.9, 8.1, 7.6, 10.1, 6.7, 7.8 s (median 8.1 s) | the same pattern: audio 1.0–2.9 s, then the 4 s GPU wait, then the server 0.6–2.5 s |
| Final, with `--progress` (what the app runs), the same session | 4.7, 4.4, 4.5, 4.7, 4.5, 4.7 s (median 4.7 s) | 5 `starting` lines per start, the first 0.2–0.5 s after the spawn, never more than 1.2 s between lines (the supervisor allows 10 s) |

Idle (other builds running): 2.9–8.9 s before. In a debug build the GPU device takes 3–8 s idle and 5–14 s under load, so a Gui engine usually starts without it; the media capabilities probe takes 2–27 s and already ran on a worker.

Two things were wrong. The engine kept optional work on the path to the ready line (the GPU window was counted from after the other devices, and the run-file's permission tools ran in it). And the supervisor treated "not ready after a fixed time" as a crash, so a slow but healthy start burned the crash budget, while a hung one could not be told from a slow one.

## Decision drivers

- A slow machine (busy, a slow GPU driver, antivirus scanning a new install) must start reliably.
- A dead engine must still count as a crash at once, and a hung one must still end in `failed` in bounded time.
- Exit 11 (the GPU failed in the start window), exit 12 handling, the handshake and the run-file keep their meaning.
- Other clients of the launch protocol (the CLI tests, agents' tooling) must not break.

## Considered options

1. **The ready line means serving, and the supervisor judges liveness** (chosen): the engine takes optional work off the path to the ready line; with `--progress` it prints `starting` lines (one when it runs, then one a second) until it is ready; the supervisor kills a start that is silent for 10 s or not ready after 60 s, and fails after four failed starts in a row.
2. **Raise the 10 s timeout** (for example to 30 s) and keep counting a timeout as a crash. Rejected as the whole fix: it waits as long for a hung engine as for a slow one, and four 30 s timeouts never fall within the 60 s crash window, so a hung engine would restart forever.
3. **Print the ready line before the GPU settles and let exit 11 come after it.** Rejected: an exit after the ready line is a crash to every supervisor; exit 11 would become a restart loop instead of the GPU error.
4. **Heartbeats on stderr.** Rejected: stderr is for people, and main does not read it.

## Decision

Accepted by the product owner on 2026-11-26 as built in the issue #143 fix.

We will make the ready line mean "serving": the server accepts connections, the run-file is written and every command works. Work nothing needs at once goes on in the background and announces itself (`engine.degraded`) when it fails: the GPU (a Gui engine waits for it only until 4 s after it began to open; the caches, the audio output and the preview ring start inside that window), the media capabilities probe, the reaping of stale run-files, and the run-file's private file, which the engine makes while its subsystems start.

We will add the launch flag `--progress`: the engine prints `{"event":"starting","pid","version","elapsed_ms"}` as soon as its own code runs, then once a second until its ready or error line, never after it. It is opt-in, so readers that expect the ready line first are unaffected. Electron main always passes it. The change is backward compatible both ways: without the flag stdout is unchanged, and a supervisor that gets no `starting` lines treats the engine exactly as the old 10 s ready timeout did. The fix therefore does not depend on this ADR being accepted; acceptance decides whether the readiness rules stay as the contract.

We will have the supervisor judge a start by liveness: a starting engine that says nothing for 10 s (from the spawn, then between its lines) is hung; one still starting 60 s after the spawn is hung too. A hung start is killed and counts as a failed start. A start that keeps reporting progress is never cut short before 60 s. An exit is a crash at once. A fourth start in a row that never got ready is `failed`, as is a fourth crash within 60 s.

**Decided by:** maintainers' review of the issue #143 fix. Pass criteria: the supervisor tests (an engine that takes 15 s but reports progress gets ready with no crash; a silent or a hung engine is killed and ends in `failed` in bounded time; a crashing engine is still `failed` after four), the engine's launch tests for `--progress`, and the measured ready times above. Fallback: drop `--progress` from main's arguments; the supervisor then behaves as before for the first 10 s (an engine that prints nothing is hung after 10 s).

## Consequences

**Positive**

- A busy or slow machine starts: the measured ready time under load fell from a median of 11 s to 4.6 s, and a slower start that reports progress is no longer a crash.
- A frozen engine is still caught in 10 s; a hung but ticking one in 60 s; a crash loop of slow starts now ends in `failed`.
- The engine log records each start's phases (`launch phases`, `engine start phases`, `subsystem start phases`), so a slow start on someone's machine can be read from the log they attach.

**Negative / costs**

- One more line type on stdout and one more launch flag (the CLI's `cutweave engine` takes it too).
- An engine that hangs while it ticks takes up to about 4 minutes to reach `failed` (four starts of 60 s), against about 45 s for a silent one.
- The GPU window still bounds the ready time (4 s in a debug build on a busy machine), because exit 11 needs it.

**Follow-ups**

- The editor could show "Starting the engine…" with the elapsed time while main reports `starting` (UI work; not part of this fix).
- The audio output still waits for its first callback before the ready line (up to 10 s on a stalled device, inside the GPU window). Attaching playback when the device arrives would take it off the path too.
- Measure a release build and a cold disk cache on the reference machines (arch §19).

## Notes

**Issue #224 (2026-11-27): the capabilities probe had come back on the way to the ready line.** WP-2F.01's proxies chose their codec in `start` from the backend's full capabilities, whose first read is the hardware probe. Measured (debug build with FFmpeg, Windows, 16 CPU hogs and two process spawners; the app's flags): the full engine was ready 10.7 to 19.0 s after the spawn (median 14.5 s, five starts), 9.2 to 17.5 s of it that probe; idle 2.3 s, 1.7 s of it the probe. The proxies now choose from the software codecs (`MediaBackend::software_capabilities`, no probe) and the rules above are unchanged. The same load, the two builds started in turn: before 11.6, 12.7, 15.4 and 16.8 s; after 0.8, 1.5, 1.8 and 2.0 s. What is left under heavy load is the OS's: on a machine busy starting other processes, loading the engine binary and the run-file's `whoami` and `icacls` each took up to about 14 s; the run-file's private file is now made as soon as the engine runs. A quit while the probe still runs waits for it, as this ADR's worker always did.

## Links

- Architecture: `../01-architecture.md` §2, §8, §19
- Launch protocol: [`../reference/launch-protocol.md`](../reference/launch-protocol.md) ("Ready means serving", "Starting lines", "Supervision")
- Issue #143; WP-1I.04 (launch protocol), WP-1I.05 (GPU wait), WP-1L.03 (supervisor)
