# Cutweave task runner (docs/06-dev-process.md §2). Run `just --list`.
# On Windows, recipes run under Git Bash.

set shell := ["bash", "-euo", "pipefail", "-c"]

default:
    @just --list

# Toolchains, pnpm install, FFmpeg bundle, hooks, corpus manifest
setup:
    rustup show active-toolchain
    pnpm install --frozen-lockfile
    @echo "setup: FFmpeg bundle, git hooks and corpus manifest arrive in WP-0A.06"

# Engine (cargo watch) + Electron with Vite HMR
dev:
    @echo "dev: not implemented yet (arrives in WP-1A and the Phase 1 shell work packages)"

# fmt, clippy -D warnings, nextest + doc-tests, wasm32 check, dep rule, license headers, cargo-deny, Biome, Vitest
check:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo nextest run --workspace
    cargo test --workspace --doc
    cargo xtask check-wasm
    cargo xtask check-deps
    cargo xtask license-headers --check
    cargo deny check
    cargo xtask tokens --check
    pnpm lint
    pnpm format:check
    pnpm typecheck
    pnpm test
    @echo "check: schema/codegen snapshot diff arrives in WP-1N"

# Render golden-frame projects headless and compare (`just golden --update`)
golden *args:
    @echo "golden: not implemented yet (arrives in WP-0I)"

# Criterion + engine benchmarks against the budgets (`just bench --compare main`)
bench *args:
    @echo "bench: not implemented yet (arrives in WP-0I)"

# Playwright + Electron flows
e2e:
    @echo "e2e: not implemented yet (arrives in Phase 1)"

# Start the headless engine and run the MCP conformance/inspector suite
mcp-inspect:
    @echo "mcp-inspect: not implemented yet (arrives in WP-1N)"

# Fetch the test-media corpus (not committed; checksummed)
corpus:
    cargo xtask fetch-corpus

# Run Agent Kit eval tasks (`just evals --client claude`)
evals *args:
    @echo "evals: not implemented yet (arrives in Phase 6)"

# Download the pinned CI-built FFmpeg shared bundle and set FFMPEG_DIR
ffmpeg-fetch:
    @echo "ffmpeg-fetch: not implemented yet (arrives in WP-0A.06)"
