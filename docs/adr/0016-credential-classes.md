# ADR-0016: Credential classes (UI / agent / plugin), server-derived actor and UI-only scopes

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §6, §13 (spending authority), §14 (safety), §18

## Context

The engine runs a localhost server used by the desktop UI, agents (MCP/CLI) and plugins. A discovery file (`~/.cutweave/run/<pid>.json`) lets agents attach, so any process that can read it can talk to the engine. Some operations must be reserved for a human: entering API keys, approving spending or destructive actions, changing filesystem allowlists and budgets, and granting plugin permissions. An agent that can mint its own approvals defeats the three-tier spending model (ADR-0008). Review item R20 fixed this.

## Decision drivers

- An agent, or any process with access to the run-file, cannot approve its own spending or widen its own access.
- Attribution of every edit to the right actor, for per-actor undo and audit.
- No credential for the UI on disk.
- Plugins limited to what they declared.

## Considered options

1. **Three credential classes, actor derived server-side, UI-only scopes only for the UI credential** (chosen).
2. **One token for all clients.** Rejected: a run-file reader could approve spending and set keys.
3. **Client-asserted actor labels.** Rejected: spoofable; per-actor undo and audit would be meaningless.
4. **OS-level auth (peer credentials, OAuth).** Rejected for v1: portability, complexity, and remote access is out of scope.

## Decision

- **UI credential:** a per-launch 256-bit token handed to Electron main over the engine's spawn-handshake pipe, **never written to disk**, and delivered to the renderer via preload. It maps to actor `user` and is the only credential allowed the **UI-only scopes**: `ai.set_key`, minting spend and destructive approvals, changing filesystem allowlists and budgets, and granting plugin permissions.
- **Agent credentials:** tokens in the run-file, used by MCP and CLI clients. They map to `agent:<label>` and can never use UI-only scopes.
- **Plugin credentials** are issued in-process per plugin: actor `plugin:<id>`, restricted to its declared permissions.
- The **actor is derived server-side from the credential**, never asserted by the client.
- Network hygiene: localhost-only bind, Host and Origin header checks (DNS-rebinding defense), stable port chosen by Electron main (R24). Remote access is out of scope for v1.
- Headless mode can only use spending tiers 1-2 with an explicit `--budget` (ADR-0008).

## Consequences

**Positive**

- Prompt-injected or malicious agents cannot approve their own spending or exfiltrate keys, because keys are write-only and the approval scope is not theirs.
- Per-actor history ("undo the agent's last change") is trustworthy.
- The security model is simple to state and test.

**Negative / costs**

- The handshake and token handoff must be implemented carefully (spawn pipe, preload bridge). The renderer holds the UI token in memory, so renderer compromise equals UI-level compromise; hence the sandboxing and CSP requirements (arch §18, ADR-0001).
- Headless operation cannot approve human-gated actions; automation must pre-set budgets.
- Run-file permissions (user-only) must be enforced per OS, and the file must be cleaned up on exit and crash.

**Follow-ups**

- Epic 1I (auth), 5A (policy engine), plugin permission prompts in 6A; security review in 7F.

## Links

- Architecture: [§6](../01-architecture.md), [§13](../01-architecture.md), [§14](../01-architecture.md), [§18](../01-architecture.md)
- Research: [05-mcp-integration.md](../research/05-mcp-integration.md) §4.2 (safety, scopes), [06-cloud-ai-providers.md](../research/06-cloud-ai-providers.md) §5.4 (key storage)
- Integrator resolutions: R9, R18, R20, R24 in [05-work-breakdown.md](../05-work-breakdown.md)
- Related: ADR-0002, ADR-0005, ADR-0008, ADR-0017
