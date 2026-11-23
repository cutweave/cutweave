# Engine launch protocol

How a supervisor (Electron main's `EngineSupervisor`, the CLI, a test) starts an engine process, learns that it is ready, and tells why it stopped. Implemented in `cutweave_engine::launch` (WP-1I.04); the token rules, the readiness line and the port contract come from WP-1I.01 (`cutweave_api::launch`). Architecture: [§2](../01-architecture.md#2-process-model), [§6](../01-architecture.md#6-api-surfaces-cutweave-api), [§18](../01-architecture.md#18-security--privacy); credentials: [ADR-0016](../adr/0016-credential-classes.md).

## Command line

```text
cutweave engine [--headless] --port N [--token-stdin] [--project P] [--run-dir D]
                [--allow-origin O]... [--allow-root R]... [--deterministic-ids SEED]
                [--log-dir D | --no-log] [--handshake-timeout-ms MS]
```

Until `cutweave engine` lands (WP-1K.01) the same entry point is the `cutweave-engine-serve` binary of `cutweave-engine` (point `CUTWEAVE_ENGINE_BIN` at it).

| Flag | Meaning |
|---|---|
| `--port N` | Bind `127.0.0.1:N`. Electron main chooses the port on every (re)start so the renderer's CSP stays valid; `0` picks a free port. |
| `--headless` | Headless mode: no UI credential class at all (agents and the CLI use the run-file's agent tokens). Without it the engine runs in **Gui** mode. |
| `--token-stdin` | Accepted for compatibility: a Gui engine always reads its UI credential from stdin. With `--headless` it is a bad handshake (exit 13). |
| `--project P` | Open this project folder or `.cutweave` file at start (exit 10 if another process holds it). Without it the engine starts with an untitled document. |
| `--run-dir D` | Run-file directory (default `~/.cutweave/run`). |
| `--allow-origin O` | An extra allowed `Origin` (the launcher passes `app://cutweave`). Repeatable. |
| `--allow-root R` | A folder agents and headless callers may import media from, besides the project folder. Repeatable. |
| `--deterministic-ids SEED` | Seeded document ids, separately seeded job ids, clock frozen at the Unix epoch (tests; WP-1B.08 adds the env var and refuses it in the GUI). |
| `--log-dir D` / `--no-log` | Log directory (default `~/.cutweave/logs`), or no log file. |
| `--handshake-timeout-ms MS` | How long a Gui engine waits for the handshake line (default 5000; tests shorten it). |

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
| 11 | `gpu_init_failed` | The GPU could not be initialized (Gui mode; WP-1I.05) | Show the GPU error |
| 12 | `port_in_use` | The port could not be bound | Pick a new port, update the CSP, restart |
| 13 | `bad_handshake` | Handshake missing, late, malformed, or given to a headless engine | Bug in the supervisor |

## Logs and crash marker

- **Log file:** `~/.cutweave/logs/engine-<pid>.log`, one JSON object per line (`ts`, `level`, `target`, `msg`, `fields`, `spans`, `thread`). Rotated at 10 MB; the directory keeps the newest 5 engine log files.
- **Crash marker:** a panic writes `crash-<pid>.json` (`{"event":"crash","pid","version","ts","thread","message","location"}`) next to the log. A clean exit removes it, so a marker left behind by a dead engine means it crashed (WP-1L.03 shows it).
- **No telemetry:** the engine opens no outbound connection; it only listens on `127.0.0.1`.
