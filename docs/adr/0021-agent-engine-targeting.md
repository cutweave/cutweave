# ADR-0021: Agents choose among several engines by project, and default to the focused window's engine

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** product owner (request, WP-1J.04), maintainers (accept)
- **Supersedes:** none (extends the run-file of the launch protocol and the attach proxy of ADR-0009)
- **Architecture refs:** `docs/01-architecture.md` §2, §6, §14; `docs/reference/launch-protocol.md` (The run-file); `docs/user-flows.md` F30

## Context

The app and every `cutweave engine --headless` a script starts are separate engines, each with a run-file. An agent configured with plain `cutweave mcp --attach` found the "one" engine, and with two or more it failed asking for `--pid`, a number that changes on every start and cannot sit in a client's config. The product owner asked: with three instances of Cutweave running, how does the agent know which one to control?

## Decision

- **The run-file says what a chooser needs:** `project` (`{name, path}`, kept current as projects open and close), `mode` (`gui` or `headless`), `started`, and for GUI engines `focused_at` (milliseconds). Run-file writes are atomic (temp file and rename); stale files stay ignored by the pid check.
- **Choosing:** `--project <name or path>` (case-insensitive name, project folder or `.cutweave` file) or `--pid` on `exec`, `query` and `mcp --attach`. With neither: the GUI engine focused last; else the only engine; several headless engines are exit 3 with the list. `cutweave engines [--json]` lists them, newest focus first.
- **Focus is reported by the app:** Electron main sends `project.report_focus` when the window gains the focus. It is an engine command in the UI-only scope (ADR-0016), `mcp = "hidden"`: it is the app applying its own state to its engine, no agent can call or see it. The engine stamps its own clock and rewrites the run-file.
- **The attach proxy has two tools of its own**, `engines_list` and `engine_attach{project|pid}`. They are added to the engine's `tools/list` by the proxy, answered by the proxy and never sent to an engine, so the engine's tool budget (the P1 gate) is unchanged. `engine_attach` opens the new upstream session first (replaying the client's `initialize`), then drops the old one, and sends `notifications/tools/list_changed` when the engine's tool list differs from what the client last saw.
- **`--project` keeps its old meaning as a fallback:** a path that exists and that no running engine has open starts a one-shot engine on it, as before; `mcp --attach` never does.

## Consequences

- One config (`cutweave mcp --attach`) serves one engine or many, and by default drives the project the person is looking at.
- The proxy's `tools/list` is the engine's plus two lean tools (about 0.2k tokens). Clients that count tools must count them apart from the engine's gate.
- A project rename reaches the run-file at the next save; the folder name also matches meanwhile.
- Out of scope: several windows in one app (one engine per window, later), engines on other machines.
