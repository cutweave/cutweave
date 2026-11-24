# Engine launch protocol

How a supervisor (Electron main's `EngineSupervisor`, the CLI, a test) starts an engine process, learns that it is ready, and tells why it stopped. Implemented in `cutweave_engine::launch` (WP-1I.04); the token rules, the readiness line and the port contract come from WP-1I.01 (`cutweave_api::launch`). Architecture: [§2](../01-architecture.md#2-process-model), [§6](../01-architecture.md#6-api-surfaces-cutweave-api), [§18](../01-architecture.md#18-security--privacy); credentials: [ADR-0016](../adr/0016-credential-classes.md).

## Command line

```text
cutweave engine [--headless] --port N [--token-stdin] [--project P] [--run-dir D]
                [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED]
                [--i-know-this-is-a-test] [--log-dir D | --no-log] [--handshake-timeout-ms MS]
                [--dev-stub-media]
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
| `--dev-stub-media` | **Dev only.** Attaches a stand-in media prober instead of the full engine (FFmpeg media, the GPU, audio and playback; WP-1I.05), so tests and UI work without the FFmpeg bundle can `media.import` files and put clips on the timeline. Any existing, readable file is "probed" from its extension alone: video (`mp4 mov mkv webm m4v avi`) is 1920x1080, 30 fps, 10 s with a 48 kHz stereo audio stream; audio (`wav mp3 aac m4a flac ogg opus`) is 48 kHz stereo, 10 s; images (`png jpg jpeg webp gif`) are 1920x1080 stills. Other extensions go to `failed[]` (`unsupported`), missing files to `failed[]` (`not_found`). Nothing is decoded, so there is no preview or export of real pixels. The data is marked as made up: every stream's codec is `"stub"` and the asset's probe carries `"stub": true`. The content hash is SHA-256 over the file length and its first and last 64 KiB (`stub-sha256:...`). The environment variable `CUTWEAVE_DEV_STUB_MEDIA=1` does the same (the flag wins); the engine logs a warning at startup when it is on. Without the flag nothing changes: imports report every file as unreadable. |

## Sequence

1. **Handshake (Gui mode).** The supervisor writes the per-launch UI credential (a 256-bit token, 32–512 printable ASCII characters) as the **first line on stdin**. The token is never passed in argv or the environment and never written to disk or logs. No line within 5 s, EOF first, or a malformed token: exit 13.
2. **Start.** The engine opens `--project` (if any), starts the document actor, journal, autosave, job manager and subsystems, binds the port and writes the run-file (agent tokens only, user-only permissions).
3. **Ready.** Exactly one line on stdout:

   ```json
   {"event":"ready","port":4321,"pid":12345,"version":"0.0.0"}
   ```

   It never contains a token. The supervisor then connects to `ws://127.0.0.1:<port>/rpc` with the UI credential.
4. **Run.** In Gui mode the engine keeps reading stdin and **stops when stdin closes** (the supervisor closed the pipe or died: no orphaned engines). Any engine also stops on Ctrl-C / SIGINT / SIGTERM.
5. **Stop.** The server stops and the run-file is removed, the journal is synced and closed, the project lock released. **Nothing is saved implicitly**: unsaved work stays in the journal and is offered for recovery on the next open. Exit 0.

Stdout carries JSON lines only. A failed start prints one error line before exiting:

```json
{"event":"error","code":"project_locked","exit_code":10,"message":"the project is already open in another Cutweave process (pid 4242 on HOST, since unix time 1763900000)","holder":{"pid":4242,"host":"HOST","started":1763900000}}
```

Human-readable diagnostics go to stderr; structured logs go to the log file.

## Exit codes

| Code | `code` in the error line | Meaning | Supervisor action |
|---|---|---|---|
| 0 | | Clean stop (stdin closed or signal) | None |
| 1 | `start_failed`, `project_open_failed`, `server_failed`, `subsystem_failed` | Any other failure | Show the error, offer a restart |
| 2 | `usage` | Bad arguments | Bug in the supervisor |
| 10 | `project_locked` | The project is open in another process | Tell the user who holds it |
| 11 | `gpu_init_failed` | The GPU could not be initialized within 4 s of the start (Gui mode; WP-1I.05). A later failure does not exit: the engine falls back to software rendering and sends `engine.degraded`. A headless engine never exits 11 | Show the GPU error |
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

**Writes are atomic.** Every write goes to `<pid>.tmp` (user-only from the first byte) and is renamed over `<pid>.json`, so a reader sees the old file or the new one and never none or half of one. The engine serializes its rewrites (project, focus, ring) so none loses another's field, and never recreates the file once shutdown removed it. **Stale files** (an engine killed with `kill -9`) are ignored by readers when their pid is not running, and reaped at the next engine start.

**Focus.** `project.report_focus` is an engine command in the `ui` scope (UI credential only, ADR-0016) and `mcp = "hidden"`: no agent can call it or see it. Electron main sends it on every `focus` of the window, and once after the control connection opens (also after a supervised restart) if the window is focused then. The engine stamps its own clock and rewrites `focused_at`.

**Choosing.** Newest `focused_at` first; with `--project` the engine with that project (name case-insensitive, or a project folder or `.cutweave` path); with neither, the GUI engine focused last, else the only engine, else exit 3 with the list.

## Logs and crash marker

- **Log file:** `~/.cutweave/logs/engine-<pid>.log`, one JSON object per line (`ts`, `level`, `target`, `msg`, `fields`, `spans`, `thread`). Rotated at 10 MB; the directory keeps the newest 5 engine log files.
- **Crash marker:** a panic writes `crash-<pid>.json` (`{"event":"crash","pid","version","ts","thread","message","location"}`) next to the log. A clean exit removes it, so a marker left behind by a dead engine means it crashed (WP-1L.03 shows it as `lastError.panic`).
- **No telemetry:** the engine opens no outbound connection; it only listens on `127.0.0.1`.

## Supervision (Electron main)

Every build of the desktop app passes the folders of Settings > Agents and MCP (`agents.allowed_folders`, [settings.md](settings.md)) as `--allow-root` on every engine start and supervised restart; a stored folder that no longer exists is skipped with a warning in the main log (it stays stored). When the list changes, main sends it to the running engine with `project.set_allowed_roots` on its own connection, and the change reaches the engine before the Settings call returns; an engine that was starting meanwhile gets it as soon as main is connected. Dev builds (not packaged) also add every folder in `CUTWEAVE_ENGINE_ALLOW_ROOTS` (a `;`-separated list on Windows, `:` elsewhere), first, so agents attached to a dev engine can import from and save into those folders. `CUTWEAVE_DEV_STUB_MEDIA=1` is inherited by the engine and turns on the stub media backend. Two more variables reach the engine: `CUTWEAVE_GPU` (`auto`, `software` or an adapter name) picks the GPU, and `CUTWEAVE_AUDIO` (`null`, `default` or a device name) picks a Gui engine's audio output. A headless engine always plays on the null clock.

`EngineSupervisor` in `apps/desktop/src/main/engine/supervisor.ts` (WP-1L.03) applies this protocol:

- **Per launch:** the port main chose at app start (`--port`, so the renderer's CSP stays valid), a **new UI token for every engine process** (stdin only; the renderer fetches it again after a restart), `--allow-origin app://cutweave`, `--project` when a project was open, and an `--allow-root` for each folder agents may use, read anew at every (re)start. The ready line has 10 s.
- **States:** `starting → ready → crashed → restarting → ready`, plus `failed` and `stopped`. An exit after the ready line is a crash: restarts follow after 0.5 s, 1 s and 2 s; a fourth crash within 60 s is `failed`, and main asks Restart / Quit. Exit 12 picks a new port, restarts at once and reloads the editor with the new CSP; before every restart the old port is checked and replaced if taken. Exits 2, 10, 11 and 13 are `failed` at once; exit 1, a signal or a missing ready line count as crashes.
- **Main's own connection:** after the ready line main connects to `/rpc` with the UI token, subscribes to the project events and reads `rpc.hello`, so it always knows which project to reopen. The engine is reported `ready` (and the renderer gets its credentials) only after that.
- **Window focus:** main reports each focus gain of the window with `project.report_focus` (see [The run-file](#the-run-file)), and tells a fresh engine at once when the window is already focused.
- **Supervised restart vs. cold open:** a supervised restart reopens the project the crashed engine held and main calls `project.recover` before reporting `ready`: the journal is replayed with **no prompt**, because it holds the work of the session the user was in seconds ago. A user's cold open of a project with unsaved work (`project.open`) instead gets `project.recovery_available` and the restore/discard dialog (WP-1M.07). If that dialog was still undecided when the engine crashed, the restart leaves the decision to the user. An untitled document is not journaled; a restart brings back an empty one.
- **Quit:** stdin is closed (and SIGTERM sent on POSIX); after 3 s the engine is killed. Because a Gui engine stops on stdin EOF, an engine never outlives a crashed or killed Electron main.
- **Test hook:** debug engine builds abort right after their Nth committed document command when `CUTWEAVE_TEST_CRASH_AFTER=N` is set (the reply is never sent; the journal holds the commit). Release builds ignore it.
