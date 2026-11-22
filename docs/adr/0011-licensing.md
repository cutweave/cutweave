# ADR-0011: GPL-3.0 core; MIT OR Apache-2.0 for the plugin SDK, WIT and compose runtime; plugin exception; DCO

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved); decision made by the product owner on 2026-11-22 (product brief D4)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §15 (Licensing), §21 (dependency and model policy), §12 (license hygiene)

## Context

The product is "fully open source" (brief D4). Plugins must be attractive to authors, including commercial ones, yet the editor itself must not be forkable into a closed product. A GPL host raises the question of whether plugins and SDK users are derivative works. Research 07 §8 compares four options. This is not legal advice; a named legal reviewer checks the exception text and codec posture (WP-0B.01, day-0 item D0.6).

## Decision drivers

- Copyleft for the editor and engine.
- Clear, cheap rights for plugin authors who use only published interfaces.
- No ambiguity about the SDK, which is the layer a plugin actually links or imports.
- A contributor mechanism that works from day one.
- Dependencies and model weights must be GPL-3.0-compatible.

## Considered options

1. **GPL-3.0-or-later host + permissive SDK/WIT/compose-runtime (MIT OR Apache-2.0) + written plugin exception + DCO** (chosen; research 07 option B).
2. **Pure GPL-3.0 everywhere** (option A). Rejected: chills commercial and proprietary plugin packs and leaves SDK-linking plugins ambiguous.
3. **LGPL/MPL engine crates + GPL GUI** (option C). Rejected: allows closed forks of the engine, contrary to the open-source goal.
4. **No explicit statement** (option D). Rejected: ambiguity deters authors.
5. **CLA instead of DCO.** Rejected: a DCO has lower contributor friction. Research 07 notes that a DCO cannot add exceptions later, so the exception ships from day one and relicensing is not planned.

## Decision

- Core (engine, crates, apps, packages other than the three below) is **GPL-3.0-or-later**.
- `wit/`, `packages/plugin-sdk` and `packages/compose-runtime` are **MIT OR Apache-2.0**. These are the only plugin-facing code and interface surfaces.
- A **written plugin exception** states that plugins using only the published interfaces (WIT worlds, command bus/CLI/MCP, effect and composition manifests, iframe RPC) are independent works under any license. Native code is never loaded in-process (ADR-0010 L4), which supports the separate-works argument.
- Every commit carries a **DCO** sign-off (`git commit -s`); there is no CLA.
- SPDX identifiers are mandatory in source headers (`GPL-3.0-or-later`, or `MIT OR Apache-2.0` in the three permissive areas), enforced by `cargo xtask license-headers` and REUSE.
- Dependency and model policy (arch §21): allowed MIT, BSD, Apache-2.0, ISC, Zlib, MPL-2.0, LGPL-2.1+/3.0, GPL-2.0-or-later, GPL-3.0, CC0/CC-BY; **not** allowed GPL-2.0-only, AGPL, non-commercial/research-only, unlicensed weights. Enforced by `cargo-deny`, the JS license check and catalog metadata.
- Remotion code and API are not copied (brief D8).

## Consequences

**Positive**

- Authors can ship proprietary or commercial plugins that use only the permissive SDK and interfaces.
- The editor stays copyleft; the full GPL FFmpeg build (x264/x265) can be bundled.
- The license posture is machine-checkable in CI.

**Negative / costs**

- The exception text is a draft pending legal review and is **not in effect** until the legal reviewer signs off (see [LICENSE-PLUGIN-EXCEPTION.md](../../LICENSE-PLUGIN-EXCEPTION.md)); it must be finalized before 1.0 (long-lead item R17).
- Contributions to the three permissive directories are made under both licenses, so reviewers must route changes carefully and no GPL-only code may be copied into them.
- GPL-2.0-only and AGPL dependencies are off the table, which excludes some otherwise useful libraries.
- Codec patents remain a separate legal topic (arch §21).

**Follow-ups**

- WP-0B.01 (license text and exception), WP-0B.02 (permissive packaging and REUSE), Epic 7F (legal and compliance review).

## Links

- Architecture: [§15](../01-architecture.md), [§21](../01-architecture.md), [§12](../01-architecture.md)
- Research: [07-plugin-architecture.md](../research/07-plugin-architecture.md) §8 (licensing options A-D), [04-local-ai.md](../research/04-local-ai.md) (model license traps)
- Repository outputs of WP-0B.01 / 0B.02: [LICENSE](../../LICENSE), [LICENSE-PLUGIN-EXCEPTION.md](../../LICENSE-PLUGIN-EXCEPTION.md), [REUSE.toml](../../REUSE.toml), [CONTRIBUTING.md](../../CONTRIBUTING.md), [GOVERNANCE.md](../../GOVERNANCE.md)
- Product brief: [D4 and D8](../00-product-brief.md)
- Related: ADR-0010
