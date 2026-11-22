# ADR-0002: Ship one `cutweave` binary with a headless-first engine and a single command bus

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §1, §2, §5, §6, §14

## Context

The product brief ([00-product-brief.md](../00-product-brief.md), principle 1 and decision D5) requires that anything the UI can do an agent can do, with no screenshots. The editor must also run in CI and on servers (`cutweave render`, `cutweave mcp --headless`), and a GPU, decoder or plugin crash must not lose user work. We need a process and API shape that makes parity structural instead of a testing discipline.

## Decision drivers

- Agent/UI/CLI parity by construction (D5).
- Crash isolation between UI and engine; recovery from a journal (arch §17).
- Headless operation for CI, servers, MCP and parallel export.
- A UI-agnostic engine, so a shell swap (ADR-0001) costs only the UI.
- One artifact to build, sign and ship.

## Considered options

1. **One `cutweave` Rust binary with modes; engine is a headless daemon; every client talks to one command bus** (chosen).
2. **Engine linked in-process into the UI shell** (Electron native addon or Tauri core). Rejected: a GPU, decoder or plugin crash takes down the UI and the unsaved session. Headless use would need a second code path, and parity would depend on discipline.
3. **Separate binaries per role** (daemon, CLI, MCP server, worker). Rejected: version skew between binaries, more to sign and package, and duplicated wiring. One binary with subcommands gives the same isolation.
4. **UI-owned state with the engine as a rendering library.** Rejected: it makes the UI the source of truth and breaks agent parity and replayability (brief principles 1-3).

## Decision

`cutweave` is a single Rust executable with modes: `engine` (the daemon the GUI spawns), `mcp --attach`, `mcp --headless`, `exec`/`query`, `render` and the internal `worker`. The engine runs as a child process of Electron main, which supervises and restarts it. The UI, MCP server, CLI and plugins are all clients of the same **command bus** in the engine; every mutation is a registered command applied through a transaction (ADR-0005). The project document lives only in the engine; the UI holds a read-only projection.

## Consequences

**Positive**

- Parity is structural. A CI test enumerates the registry and asserts every non-`view` command is reachable over MCP with the same schema as RPC (arch §14, §20).
- Engine crashes do not take down the UI. The main process restarts the engine, which replays the journal; the UI resyncs from a snapshot.
- Headless CI renders, `mcp --headless` and parallel export workers reuse the same binary and code path.
- Client parity tests can byte-compare documents across CLI, MCP and RPC (needs ADR-0015 deterministic IDs).

**Negative / costs**

- Every UI interaction crosses a process boundary: command round trip budget p95 < 16 ms (arch §19) and a resync protocol after engine restart.
- Preview frames need a shared-memory path instead of in-process textures (arch §10, ADR-0001).
- A local server needs auth, Host/Origin checks and a stable port (ADR-0016, R24).
- The binary grows with every mode; modes must stay thin.

**Follow-ups**

- Crate boundaries and the dependency rule are CI-enforced (`cargo xtask check-deps`, arch §3).
- Engine supervision and resync land in Epic 1L; the local API in Epic 1I.

## Links

- Architecture: [§1](../01-architecture.md), [§2](../01-architecture.md), [§5](../01-architecture.md), [§6](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) (state architecture §6.2), [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (single command bus)
- Integrator resolutions: R10, R19, R24 in [05-work-breakdown.md](../05-work-breakdown.md)
- Related: ADR-0001 (shell), ADR-0005 (state), ADR-0009 (MCP), ADR-0015 (wire protocol), ADR-0016 (credentials)
