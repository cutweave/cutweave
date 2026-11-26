# Architecture Decision Records

ADRs record architectural decisions in [MADR](https://adr.github.io/madr/) style: context, options, decision, consequences. Process: [docs/06-dev-process.md](../06-dev-process.md) §9. Status flows **Proposed → Accepted** (or *Accepted with fallback*, *Rejected*, *Superseded by ADR-NNNN*). The orchestrator may draft ADRs; only maintainers accept them. Contracts (command schemas, WIT, project format, plugin manifest, MCP surface) change only via ADR.

- Template: [0000-template.md](0000-template.md). File names are `NNNN-slug.md`, with four-digit numbers matching the decision log in [docs/01-architecture.md](../01-architecture.md) §23.
- A **Proposed** ADR names the spike or work package that decides it, with pass criteria. It becomes Accepted only in WP-0J.02 or WP-0J.03 ([docs/wbs/P0-foundations.md](../wbs/P0-foundations.md)).
- When a status changes, update this table and ask the integrator to update arch §23 (list deltas in `ARCH-DELTAS.md`, WP-0J.02).

| ADR | Decision | Status | Decided by |
|---|---|---|---|
| [0001](0001-desktop-shell.md) | Desktop shell: Electron + React, separate Rust engine, preview via SHM; GPUI plan B | Proposed | Spike S1 (WP-0C.04, WP-0J.02) |
| [0002](0002-single-binary-headless-engine.md) | Single `cutweave` binary, headless-first engine, one command bus | Accepted | n/a (2026-11-22) |
| [0003](0003-media-backend-ffmpeg.md) | Media: FFmpeg via `ffmpeg-next` behind `MediaBackend`; hardware decode where exact, CPU upload, proxies for long GOPs | Accepted with fallback | Spike S3 (WP-0E.01, WP-0E.02 on Windows); macOS and Linux pending (0E.03) |
| [0004](0004-flicks-time-unit.md) | Time unit: flicks (`i64`) | Accepted | n/a (2026-11-22) |
| [0005](0005-doc-actor-persistent-snapshots-patches.md) | State: single-writer doc actor, `imbl` snapshots, invertible patches; CRDT later | Accepted | n/a (2026-11-22) |
| [0006](0006-project-format.md) | Project format: folder + JSON doc + journal + backups | Accepted | n/a (2026-11-22) |
| [0007](0007-text-rendering.md) | Text: cosmic-text + swash + MSDF; Vello variant | Proposed | Spike S4 (WP-0F.03) |
| [0008](0008-ai-gateway.md) | AI: capability-typed gateway, signed catalog, declarative HTTP and WASM adapters, cloud-first | Accepted | n/a (2026-11-22) |
| [0009](0009-mcp-server.md) | MCP: `rmcp`, hybrid catalog, attach + headless | Proposed | Spike S5 (WP-0H.03) |
| [0010](0010-layered-plugin-system.md) | Plugins: layered L0-L4; wasmtime + WIT; QuickJS compositions; WGSL effects | Accepted | n/a (2026-11-22) |
| [0011](0011-licensing.md) | License: GPL-3.0 core, MIT/Apache SDK/WIT/compose-runtime, plugin exception, DCO | Accepted | Product owner (2026-11-22) |
| [0012](0012-compositions-runtime.md) | Compositions: TS/JSX → oxc → QuickJS → scene graph | Proposed | Spike S4b (WP-0G.02) |
| [0013](0013-ui-stack.md) | UI: React 19 + Zustand + Canvas2D timeline + dockview + schema-driven inspector | Accepted | n/a (2026-11-22) |
| [0014](0014-min-os.md) | Minimum supported OS versions | Proposed | WP-0J.03 (Phase 0) |
| [0015](0015-wire-protocol.md) | Wire protocol: `request_id` idempotency, flicks as JSON integers, deterministic IDs | Accepted | n/a (2026-11-22) |
| [0016](0016-credential-classes.md) | Credential classes (UI / agent / plugin), server-derived actor, UI-only scopes | Accepted | n/a (2026-11-22) |
| [0017](0017-transaction-kinds.md) | Transaction kinds (`gesture`, `proposal`), connection-owned, committed-only saves | Accepted | n/a (2026-11-22) |
| [0018](0018-user-flow-e2e-suite.md) | User-flow e2e suite (real app, real input, real dialogs) as the regression gate | Accepted | Product owner (2026-11-24) |
| [0019](0019-ui-undo-covers-every-actor.md) | The editor's Undo and Redo cover every actor's changes (`edit.undo{actor:"*"}`) | Accepted | Product owner (2026-11-24) |
| [0020](0020-media-folders.md) | Media folders: a folder tree in the project document, one folder per asset, `media.*` folder commands, schema v2 | Accepted | Product owner (2026-11-25) |
| [0021](0021-agent-engine-targeting.md) | Agents choose among several engines by project; default is the focused window's engine; `engines_list` and `engine_attach` on the attach proxy | Accepted | Product owner (2026-11-24) |
| [0022](0022-source-preview.md) | The preview plays a media item outside any sequence: one transport per document with a sequence or source target (`view.set_source`, `playback.state.source`) | Accepted | Product owner (2026-11-24) |
| [0023](0023-timeline-edit-semantics.md) | Timeline edit semantics for P2: magnetic main track, placement modes, attachments (`attached_to`), `EditReport`, gesture updates that replace (`_meta.replace`) | Accepted | Product owner (2026-11-25) |
| [0024](0024-keyframes.md) | Keyframes: item-relative flicks, per-segment easing (hold, linear, CSS-style bezier, physical spring), exact split and trim rules, `keyframe.*` commands; spec in `docs/spec/keyframes.md` | Accepted | Product owner (2026-11-25) |
| [0025](0025-engine-readiness-by-liveness.md) | The engine's ready line means serving (optional work after it); `--progress` `starting` lines; the supervisor judges a start by liveness (10 s silent or 60 s starting is hung; four failed starts in a row is `failed`) | Accepted | Product owner (2026-11-26) |
| [0026](0026-context-menu-surfaces-and-contributions.md) | Context menus: in-app, built from commands, per named surface; plugins contribute to the same surfaces | Accepted | Product owner (2026-11-25) |
| [0027](0027-proxy-policy-in-the-project.md) | Proxy policy in the project (`settings.proxy_policy`) and a proxy mode per asset, optional fields without a schema bump, `media.set_proxy_policy` / `media.set_proxy_mode` | Accepted | Product owner (2026-11-26) |
| [0028](0028-mcp-result-schemas.md) | `cutweave_describe_command` serves the output schema of the MCP result, not the RPC one: untrusted strings as their `{untrusted, text}` wrapper, no `render_frame` picture; generated from the same tables that shape the results | Accepted | Product owner (2026-11-25) |
| [0029](0029-opening-a-project-never-drops-unsaved-work.md) | `project.open` and `project.close` never drop unsaved changes unasked: `Conflict{reason: unsaved_changes}` unless `discard_unsaved: true` / `save: false`; `project.opening` and `project.open_failed` frame every open, and the app shows it | Accepted | Product owner (2026-11-25) |
| [0030](0030-export-queue-in-the-project.md) | The export queue lives in the project folder (`.cutweave/exports.sqlite`, SQLite): queued exports and their document snapshots survive a restart; an untitled document's queue is in memory and moves into the project it is saved as; switching projects suspends the old one's exports | Accepted | Product owner (2026-11-26) |
| [0031](0031-detached-panels-popout-windows.md) | Detached panels are dockview popout windows in the editor's renderer process: `sandbox: false`, no preload, opened only by an editor window; part of the workspace layout | Accepted | Product owner (2026-11-26) |
| [0032](0032-export-encodes-bt709.md) | Exports encode the BT.709 transfer curve their files are tagged with; `render.frame` gets a matching transfer parameter so preview == export still holds | Accepted | Product owner (2026-11-26) |
