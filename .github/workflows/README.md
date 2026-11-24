# CI workflows

Workflows are authored in WP-0A.03. Third-party actions are limited to `actions/*`,
`dtolnay/rust-toolchain`, `Swatinem/rust-cache` and `taiki-e/install-action`, each pinned by full
commit SHA with a version comment (Renovate keeps the pins current).

## Required status checks (branch protection)

| Check | Notes |
|---|---|
| `rust (windows-latest)`, `rust (macos-latest)`, `rust (ubuntu-24.04)` | fmt, clippy, nextest, doc-tests, dependency rule, license headers |
| `ts (windows-latest)`, `ts (macos-latest)`, `ts (ubuntu-24.04)` | Biome, typecheck, Vitest, build |
| `wasm-check` | `cargo xtask check-wasm` |
| `deny` | `cargo deny check` and `pnpm audit --audit-level high` |
| `schema-snapshot` | `cargo xtask gen --check` (no-op until WP-1N) |
| `docs-lint` | relative markdown links resolve |
| `actionlint` | workflow lint |

Informational (not required): `gui-probe (x11|wayland)`, `gpu-probe (<os>)`,
`engine-linux (ubuntu-22.04 container)`, `golden-frames (windows-latest, WARP)` (WP-1O.01: the
reference-update guard with the `golden-update` label, then the golden frames; it becomes a
required check when the M1 gate aggregates the required checks, WP-1O.04).

The `ffmpeg` cargo feature is off in CI until WP-0A.06.
