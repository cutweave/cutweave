# Engine launch protocol

How a supervisor (Electron main's `EngineSupervisor`, the CLI, a test) starts an engine process, learns that it is ready, and tells why it stopped. Implemented in `cutweave_engine::launch` (WP-1I.04); the token rules, the readiness line and the port contract come from WP-1I.01 (`cutweave_api::launch`). Architecture: [§2](../01-architecture.md#2-process-model), [§6](../01-architecture.md#6-api-surfaces-cutweave-api), [§18](../01-architecture.md#18-security--privacy); credentials: [ADR-0016](../adr/0016-credential-classes.md).

## Command line

```text
cutweave engine [--headless] --port N [--token-stdin] [--project P] [--run-dir D]
                [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED]
                [--i-know-this-is-a-test] [--log-dir D | --no-log] [--handshake-timeout-ms MS]
                [--cache-dir D] [--dev-stub-media] [--progress]
```

`cutweave engine` (WP-1K.01, [cli.md](cli.md)) calls `cutweave_engine::launch::run`. The `cutweave-engine-serve` binary of `cutweave-engine` is the same entry point without the other subcommands; the integration tests of the engine and of `@cutweave/protocol` still spawn it.

| Flag | Meaning |
|---|---|
| `--port N` | Bind `127.0.0.1:N`. Electron main chooses the port on every (re)start so the renderer's CSP stays valid; `0` picks a free port. |
| `--headless` | Headless mode: no UI credential class at all (agents and the CLI use the run-file's agent tokens). Without it the engine runs in **Gui** mode. |
| `--token-stdin` | Accepted for compatibility: a Gui engine always reads its UI credential from stdin. With `--headless` it is a bad handshake (exit 13). |
| `--project P` | Open this project folder or `.cutweave` file at start (exit 10 if another process holds it). Without it the engine starts with an untitled document. |
| `--run-dir D` | Run-file directory (default `~/.cutweave/run`). |
| `--allow-origin O` | An extra allowed `Origin` (the launcher passes `app://cutweave`). Repeatable. |
| `--allow-root R` | A folder agents and headless callers may use (import media from, open projects in, save projects into), besides the project folders below. Repeatable. The desktop app passes the folders of Settings > Agents and MCP. See [Path policy](#path-policy). |
| `--deterministic-ids SEED` | Seeded document ids, separately seeded job ids, clock frozen at the Unix epoch (so `meta.created` and every timestamp in the document are fixed): identical scripts save byte-identical documents (tests). The environment variable `CUTWEAVE_DETERMINISTIC_IDS=SEED` does the same; the flag wins. A **Gui** engine refuses it (exit 2, `usage`, before reading the handshake) unless `--i-know-this-is-a-test` is also given. |
| `--i-know-this-is-a-test` | Lets a Gui engine run with deterministic ids. Ignored otherwise. |
| `--log-dir D` / `--no-log` | Log directory (default `~/.cutweave/logs`), or no log file. |
| `--handshake-timeout-ms MS` | How long a Gui engine waits for the handshake line (default 5000; tests shorten it). |
| `--cache-dir D` | The folder the engine keeps its caches in: `cache.sqlite` (frame indexes and the thumbnail accounting), `media-cache/` (thumbnails and filmstrips) and `waveforms/`. Default: `<OS cache folder>/cutweave` (`%LOCALAPPDATA%`, `~/Library/Caches`, `$XDG_CACHE_HOME` or `~/.cache`). The environment variable `CUTWEAVE_CACHE_DIR=D` does the same; the flag wins. The desktop app passes its machine preference "Cache folder" here when it starts the engine, so a new choice applies the next time Cutweave starts and the old folder is left for the person to delete. The folder is created when it does not exist; one that cannot be created or opened makes the engine report `engine.degraded{subsystem: "cache"}` and run without those caches. |
| `--progress` | Print `starting` lines on stdout until the ready (or error) line: one as soon as the engine's own code runs, before the handshake, then one a second (see [Starting lines](#starting-lines)). A supervisor uses them to tell a slow start from a hung one. Electron main always passes it. |
| `--dev-stub-media` | **Dev only.** Attaches a stand-in media prober instead of the full engine (FFmpeg media, the GPU, audio and playback; WP-1I.05), so tests and UI work without the FFmpeg bundle can `media.import` files and put clips on the timeline. Any existing, readable file is "probed" from its extension alone: video (`mp4 mov mkv webm m4v avi`) is 1920x1080, 30 fps, 10 s with a 48 kHz stereo audio stream; audio (`wav mp3 aac m4a flac ogg opus`) is 48 kHz stereo, 10 s; images (`png jpg jpeg webp gif`) are 1920x1080 stills. Other extensions go to `failed[]` (`unsupported`), missing files to `failed[]` (`not_found`). Nothing is decoded, so there is no preview or export of real pixels. The data is marked as made up: every stream's codec is `"stub"` and the asset's probe carries `"stub": true`. The content hash is SHA-256 over the file length and its first and last 64 KiB (`stub-sha256:...`). The environment variable `CUTWEAVE_DEV_STUB_MEDIA=1` does the same (the flag wins); the engine logs a warning at startup when it is on. Without the flag nothing changes: imports report every file as unreadable. |

## Sequence

1. **Handshake (Gui mode).** The supervisor writes the per-launch UI credential (a 256-bit token, 32–512 printable ASCII characters) as the **first line on stdin**. The token is never passed in argv or the environment and never written to disk or logs. No line within 5 s, EOF first, or a malformed token: exit 13.
2. **Start.** The engine opens `--project` (if any), starts the document actor, journal, autosave, job manager and subsystems, binds the port and writes the run-file (agent tokens only, user-only permissions).
3. **Ready.** Exactly one line on stdout:

   ```json
   {"event":"ready","port":4321,"pid":12345,"version":"0.0.0"}
   ```

   It never contains a token. The supervisor then connects to `ws://127.0.0.1:<port>/rpc` with the UI credential.

   **Ready means serving** (issue #143): the server accepts connections, the run-file is written and every command works. Slow work that nothing needs at once goes on in the background and announces itself when done or failed (`engine.degraded{subsystem}`):
   - **The GPU.** It opens on its own thread from the start of the subsystems. A Gui engine waits for it until 4 s after it began to open, so a GPU that fails in that window still exits 11; the caches, the audio output and the preview ring start inside the window, not after it. A device still opening then arrives later: `render.frame` and exports wait for it, and the preview shows black frames until it comes.
   - **The media capabilities probe** (hardware encoders and decoders: seconds, 9 to 17 s measured on a busy machine) runs on a worker; `media.capabilities` waits for it. Nothing before the ready line reads the full capabilities: the proxies choose their codec from the software codecs, which the backend lists without probing (issue #224).
   - **Stale run-files** of dead engines are removed in the background.
   - **The run-file's private file** is made from the moment the engine runs, before the handshake, on its own thread (on Windows restricting it runs `whoami` and `icacls`, seconds on a machine busy starting other processes), so the server only fills it. A start that fails removes it.

   What still runs before the ready line: opening the project, the caches, the audio output (it waits for its first callback: up to 10 s on a stalled device, then the null clock) and the preview ring, all inside the GPU window. Measured (debug build, Windows, 16 CPU hogs on 16 threads): ready 4.3 to 5.1 s after the spawn, of which the GPU window is 4 s and the process load 0.2 to 1 s. With Vulkan (the GPU the user flows use) the device opened in about 1 s under that load and the engine was ready 0.7 to 2.2 s after the spawn (issue #224).
4. **Run.** In Gui mode the engine keeps reading stdin and **stops when stdin closes** (the supervisor closed the pipe or died: no orphaned engines). Any engine also stops on Ctrl-C / SIGINT / SIGTERM.
5. **Stop.** The server stops and the run-file is removed, the journal is synced and closed, the project lock released. **Nothing is saved implicitly**: unsaved work stays in the journal and is offered for recovery on the next open. Exit 0.

Stdout carries JSON lines only. A failed start prints one error line before exiting:

```json
{"event":"error","code":"project_locked","exit_code":10,"message":"the project is already open in another Cutweave process (pid 4242 on HOST, since unix time 1763900000)","holder":{"pid":4242,"host":"HOST","started":1763900000}}
```

Human-readable diagnostics go to stderr; structured logs go to the log file. The log records where each start spent its time (`launch phases`, `engine start phases` and `subsystem start phases`, as `name=ms` pairs; `launch phases` also gives `run_at_unix_ms`, when the engine's own code began, so the process load is that minus the spawn time), how long the GPU took (`GPU device and compositors open`, or `the GPU did not open`, with milliseconds) and the capabilities probe (`media capabilities probed`).

### Starting lines

With `--progress` the engine also prints, before its ready or error line:

```json
{"event":"starting","pid":12345,"version":"0.0.0","elapsed_ms":2000}
```

The first comes as soon as the engine's own code runs (`elapsed_ms` 0: the process has loaded), before it reads the handshake; then one every second until the ready or error line, never after it. `elapsed_ms` counts from that first line. The lines say that the engine is alive and still starting, not which step it is in.

**Compatibility.** The lines are opt-in: without `--progress` stdout is exactly as before (the ready or error line first), so readers that take the first line as the ready line keep working. A supervisor that passes `--progress` to an engine that prints no `starting` lines sees an engine that is silent until its ready line, which the 10 s silence limit below treats exactly like the old 10 s ready timeout.

## Exit codes

| Code | `code` in the error line | Meaning | Supervisor action |
|---|---|---|---|
| 0 | | Clean stop (stdin closed or signal) | None |
| 1 | `start_failed`, `project_open_failed`, `server_failed`, `subsystem_failed` | Any other failure | Show the error, offer a restart |
| 2 | `usage` | Bad arguments | Bug in the supervisor |
| 10 | `project_locked` | The project is open in another process | Tell the user who holds it |
| 11 | `gpu_init_failed` | The GPU could not be initialized within 4 s after it began to open, at the start of the subsystems (Gui mode; WP-1I.05). A later failure does not exit: the engine falls back to software rendering and sends `engine.degraded`. A headless engine never exits 11 | Show the GPU error |
| 12 | `port_in_use` | The port could not be bound | Pick a new port, update the CSP, restart |
| 13 | `bad_handshake` | Handshake missing, late, malformed, or given to a headless engine | Bug in the supervisor |

## Path policy

Every command that takes a filesystem path (`media.import`, `project.open`, `project.save_as`, and later export targets) checks it against the caller's credential (arch [§14](../01-architecture.md#14-mcp-server-in-cutweave-api), [§18](../01-architecture.md#18-security--privacy)):

- **UI credential** (Gui mode): unrestricted. The editor only sends paths the user picked in a dialog.
- **Every other credential** (agent tokens for MCP and the CLI, plugins, internal callers): the path must lie inside one of these folders, or the command fails with `Invalid` at the path's JSON Pointer (`/path`, `/paths/<i>`), whose message tells the agent to ask the user to add the folder in Settings > Agents and MCP (a headless engine: `--allow-root`):
  - the approved folders: those given with `--allow-root`, until `project.set_allowed_roots` replaces them (WP-1J.05);
  - the folder of the project given with `--project` (for the engine's lifetime, also after it is closed);
  - the folder of the project open right now.

  The check is per path component (ASCII case-insensitive on Windows). A path with `..` is refused, and so is a relative one. Symlinks and junctions are resolved first, including for a folder about to be created (through its deepest existing ancestor), so a link inside an allowed folder cannot reach outside it. When the folder and the path both exist, their real locations decide, so another spelling of an allowed folder (an 8.3 name, `/var` for `/private/var`, a link into it) is inside it. `project.info` reports the folders in effect as `allowed_roots`.

**Changing the folders at run time.** `project.set_allowed_roots {roots}` (WP-1J.05) is how the desktop app applies its "Folders agents can use" setting to its engine. It is in the `ui` scope (UI credential only, ADR-0016: changing filesystem allowlists), so agent tokens get `forbidden` over RPC, MCP and the CLI; it is not undoable, not journaled and never dirties the project. Each root must be an absolute path without `..` naming an existing folder; it is stored canonical (symlinks resolved, Windows' `\\?\` prefix dropped where the plain path is the same folder), and the list replaces the approved folders, the `--allow-root` ones included. The engine reads the list on every call that takes a path, so removing a folder stops access at once, also for agent connections that are already open; the open project's folder stays usable.

**Default:** without `--allow-root`, an agent of a headless or oneshot engine can use only the project it was started with (`--project`) and whatever project is open. An engine started without `--project` and without `--allow-root` accepts no path from agents (in Gui mode, until the user opens a project). Automation that creates projects or imports media from elsewhere passes `--allow-root`.

## The run-file

A server engine (not a one-shot) writes `<run-dir>/<pid>.json` when it is ready and removes it when it stops. It tells local clients how to attach and which engine is which (`cutweave engines`, `--project`, `engines_list`; [cli.md](cli.md#choosing-an-engine)). User-only permissions; **agent tokens only, never the UI credential**.

| Field | Meaning |
|---|---|
| `pid`, `port`, `version` | The engine process, its loopback port and version. |
| `agent_tokens` | `[{label, token}]` for `cli` (`/rpc`) and `mcp` (`/mcp`). |
| `mode` | `gui` for the app's engine (it was given a UI credential), else `headless`. |
| `project` | `{name, path}` of the project open now (`path` is the project folder), or absent for an untitled document. Rewritten after every open, close, save and recovery; a rename shows after the next save. A file from before this field holds a bare path and is read as a project named after its folder. |
| `started` | Start time, seconds since the Unix epoch. |
| `focused_at` | GUI engines only: when the window last gained the focus, milliseconds since the Unix epoch. Absent until it has. Written by `project.report_focus`. |
| `shm` | Name of the preview ring (WP-1I.05), if any. |

**Writes are atomic.** Every write goes to `<pid>.tmp` (user-only from the first byte) and is renamed over `<pid>.json`, so a reader sees the old file or the new one and never none or half of one. The engine makes that private `<pid>.tmp` while its subsystems start, so the write before the ready line only fills it. The engine serializes its rewrites (project, focus, ring) so none loses another's field, and never recreates the file once shutdown removed it. **Stale files** (an engine killed with `kill -9`, and its `<pid>.tmp`) are ignored by readers when their pid is not running, and reaped in the background at the next engine start.

**Focus.** `project.report_focus` is an engine command in the `ui` scope (UI credential only, ADR-0016) and `mcp = "hidden"`: no agent can call it or see it. Electron main sends it on every `focus` of the window, and once after the control connection opens (also after a supervised restart) if the window is focused then. The engine stamps its own clock and rewrites `focused_at`.

**Choosing.** Newest `focused_at` first; with `--project` the engine with that project (name case-insensitive, or a project folder or `.cutweave` path); with neither, the GUI engine focused last, else the only engine, else exit 3 with the list.

## Logs and crash marker

- **Log file:** `~/.cutweave/logs/engine-<pid>.log`, one JSON object per line (`ts`, `level`, `target`, `msg`, `fields`, `spans`, `thread`). Rotated at 10 MB; the directory keeps the newest 5 engine log files.
- **Crash marker:** a panic writes `crash-<pid>.json` (`{"event":"crash","pid","version","ts","thread","message","location"}`) next to the log. A clean exit removes it, so a marker left behind by a dead engine means it crashed (WP-1L.03 shows it as `lastError.panic`).
- **No telemetry:** the engine opens no outbound connection; it only listens on `127.0.0.1`.

## Supervision (Electron main)

The app also reads this computer's preference "Cache folder" (Settings > Cache and storage, WP-1D.08) when it starts the engine and passes a chosen folder as `--cache-dir`; nothing is passed while the preference is empty. The other machine preferences (cache limits, hardware decoding, memory for decoded frames) reach the engine after each handshake as the UI-only commands `media.set_cache_config` and `media.set_decode_policy`, and `media.clear_cache` empties a cache; an agent token calling any of them gets `forbidden`, while `media.cache_status` answers everyone.

Every build of the desktop app passes the folders of Settings > Agents and MCP (`agents.allowed_folders`, [settings.md](settings.md)) as `--allow-root` on every engine start and supervised restart; a stored folder that no longer exists is skipped with a warning in the main log (it stays stored). When the list changes, main sends it to the running engine with `project.set_allowed_roots` on its own connection, and the change reaches the engine before the Settings call returns; an engine that was starting meanwhile gets it as soon as main is connected. Dev builds (not packaged) also add every folder in `CUTWEAVE_ENGINE_ALLOW_ROOTS` (a `;`-separated list on Windows, `:` elsewhere), first, so agents attached to a dev engine can import from and save into those folders. `CUTWEAVE_DEV_STUB_MEDIA=1` is inherited by the engine and turns on the stub media backend. Two more variables reach the engine: `CUTWEAVE_GPU` (`auto`, `software` or an adapter name) picks the GPU, and `CUTWEAVE_AUDIO` (`null`, `default` or a device name) picks a Gui engine's audio output. A headless engine always plays on the null clock.

`EngineSupervisor` in `apps/desktop/src/main/engine/supervisor.ts` (WP-1L.03) applies this protocol:

- **Per launch:** the port main chose at app start (`--port`, so the renderer's CSP stays valid), a **new UI token for every engine process** (stdin only; the renderer fetches it again after a restart), `--progress`, `--allow-origin app://cutweave`, `--project` when a project was open, and an `--allow-root` for each folder agents may use, read anew at every (re)start.
- **Readiness by liveness** (issue #143): a starting engine must say something (a `starting` line, or its ready or error line) within 10 s of the spawn and then at least every 10 s; one that falls silent is hung. One still starting 60 s after the spawn is hung too, however alive it looks. A hung engine is killed and counts as a failed start (`ready_timeout`). A slow start that keeps reporting is never cut short before 60 s and costs nothing; main logs one line when a start passes 10 s, and one line per start when it is ready: the milliseconds from the spawn to the engine's first line (the process load), to its ready line and to `ready` (issue #224). An engine that exits is a crash at once, whatever it reported.
- **States:** `starting → ready → crashed → restarting → ready`, plus `failed` and `stopped`. An exit after the ready line is a crash: restarts follow after 0.5 s, 1 s and 2 s; a fourth crash within 60 s is `failed`, and main asks Restart / Quit. So is a fourth start in a row that never got ready (exits before ready and hung starts, however long each took): an engine that hangs while it reports is `failed` within about 4 minutes, a silent one within about 45 s. Exit 12 picks a new port, restarts at once and reloads the editor with the new CSP; before every restart the old port is checked and replaced if taken. Exits 2, 10, 11 and 13 are `failed` at once; exit 1, a signal or a hung start count as crashes.
- **Main's own connection:** after the ready line main connects to `/rpc` with the UI token, subscribes to the project events and reads `rpc.hello`, so it always knows which project to reopen. The engine is reported `ready` (and the renderer gets its credentials) only after that.
- **Window focus:** main reports each focus gain of the window with `project.report_focus` (see [The run-file](#the-run-file)), and tells a fresh engine at once when the window is already focused.
- **Supervised restart vs. cold open:** a supervised restart reopens the project the crashed engine held and main calls `project.recover` before reporting `ready`: the journal is replayed with **no prompt**, because it holds the work of the session the user was in seconds ago. A user's cold open of a project with unsaved work (`project.open`) instead gets `project.recovery_available` and the restore/discard dialog (WP-1M.07). If that dialog was still undecided when the engine crashed, the restart leaves the decision to the user. An untitled document is not journaled; a restart brings back an empty one.
- **Quit:** stdin is closed (and SIGTERM sent on POSIX); after 3 s the engine is killed. Because a Gui engine stops on stdin EOF, an engine never outlives a crashed or killed Electron main.
- **Test hook:** debug engine builds abort right after their Nth committed document command when `CUTWEAVE_TEST_CRASH_AFTER=N` is set (the reply is never sent; the journal holds the commit). Release builds ignore it.
