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

# Electron + Vite HMR + dev engine (target/debug/cutweave, else the fake engine); cargo watch arrives with WP-1A
dev:
    pnpm --filter @cutweave/desktop dev

# fmt, clippy -D warnings, nextest + doc-tests, wasm32 check, dep rule, license headers, cargo-deny, no-literals lint, Biome, Vitest
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
    just wbs-lint
    just lint-literals
    pnpm lint
    pnpm format:check
    pnpm typecheck
    just bridge --fixture
    python scripts/run-vitest.py
    cargo xtask gen --check
    python scripts/check-md-links.py
    just spec-check

# Base: main's first parent on main, else the merge base with main (`--base REV`; `--dry-run` plans).
# Shared-config changes (root Cargo.toml, lockfile-only, toolchain, justfile) fall back to `check`.
# Gate steps the change can affect: changed crates + their dependents, TS/docs/schema only if relevant
check-affected *args:
    python scripts/check-affected.py {{args}}

# WBS lint (offline): parse docs/wbs, check ids/deps/cycles/sizes, verify docs/wbs/wbs.json snapshot
wbs-lint:
    python scripts/wbs-to-issues/wbs_to_issues.py --check
    python -m unittest discover -s scripts/wbs-to-issues

# No literal colors, px font sizes or font stacks in packages/*/src (self-test first)
lint-literals:
    python scripts/check-no-literals.py --self-test
    python scripts/check-no-literals.py

# Storybook build + Playwright visual regression, axe, offline fonts (`just ui-snapshots --update` on Linux rewrites baselines)
ui-snapshots *args:
    # Worktrees get LFS pointer files; materialize the baselines from the shared LFS store first.
    git lfs checkout packages/ui/__screenshots__
    pnpm --filter @cutweave/ui run test:visual {{args}}

# Regenerate the committed docs/wbs/wbs.json snapshot
wbs-json:
    python scripts/wbs-to-issues/wbs_to_issues.py --write

# Sync WBS work packages to GitHub issues (`just wbs-sync --repo cutweave/cutweave --phase P0 --dry-run`)
wbs-sync *args:
    python scripts/wbs-to-issues/wbs_to_issues.py --sync {{args}}

# Golden frames (testdata/golden/README.md): the reference-update guard, then build the CLI with FFmpeg, render the six golden projects headless on the software adapter and compare with that adapter's references (`--adapter key:<set>`, `--all-adapters`, `--update --i-know --reason "..."`)
golden *args:
    . scripts/ffmpeg/env.sh && cargo xtask golden {{args}}

# Criterion + engine benchmarks against the budgets (`just bench --compare main`)
bench *args:
    cargo xtask bench --suite smoke --out bench/results.json {{args}}
    cargo xtask bench-check bench/results.json

# Build the cutweave-bridge napi addon into apps/desktop/native/ (`just bridge --release`; `--fixture` adds the Rust test writer)
bridge *args:
    node apps/desktop/scripts/build-bridge.ts {{args}}

# Playwright + Electron flows
e2e:
    @echo "e2e: not implemented yet (arrives in Phase 1)"

# The user-flow suite (ADR-0018, docs/user-flows.md): builds the engine (with FFmpeg when the bundle is set up, so the real-media flows F13, F14 and F17 run; without it they skip), the CLI, the preview addon and the app, then drives it like a person (`just e2e-flows -g F8` runs one flow)
e2e-flows *args:
    if . scripts/ffmpeg/env.sh 2>/dev/null; then cargo build -p cutweave-engine --bin cutweave-engine-serve --features ffmpeg; else echo "e2e-flows: no FFmpeg bundle (just ffmpeg-fetch): the engine is built without FFmpeg and the real-media flows skip"; cargo build -p cutweave-engine --bin cutweave-engine-serve; fi
    cargo build -p cutweave-cli --bin cutweave
    pnpm --filter @cutweave/desktop build:bridge
    pnpm --filter @cutweave/desktop build:node
    pnpm --filter @cutweave/desktop build:renderer
    { . scripts/ffmpeg/env.sh 2>/dev/null || true; } && pnpm --filter @cutweave/desktop e2e:flows {{args}}

# The flows' real-cursor steps (window drag, double-click on the title, screen pixels; Windows): they move your mouse, so run them when nobody is using the machine
e2e-real-input *args:
    cargo build -p cutweave-engine --bin cutweave-engine-serve
    cargo build -p cutweave-cli --bin cutweave
    pnpm --filter @cutweave/desktop build:node
    pnpm --filter @cutweave/desktop build:renderer
    CUTWEAVE_E2E_REAL_INPUT=1 pnpm --filter @cutweave/desktop exec playwright test e2e/flows/window-and-menu.spec.ts {{args}}

# The rmcp conformance suite and the scripted agent (both MCP revisions; stdio and HTTP). Offline; also part of `just check`.
mcp-conformance:
    cargo nextest run -p cutweave-cli --test mcp_conformance

# MCP Inspector CLI (pinned) against `cutweave mcp --headless` (stdio) and a headless engine's /mcp (HTTP). Needs Node and network (npx); not part of `just check`.
mcp-inspect:
    cargo build -p cutweave-cli
    python scripts/mcp-inspect.py

# Fetch the test-media corpus (not committed; checksummed)
corpus *args:
    cargo xtask fetch-corpus {{args}}
    cargo xtask corpus-verify --allow-unpinned {{args}}

# Run Agent Kit eval tasks (`just evals --client claude`)
evals *args:
    @echo "evals: not implemented yet (arrives in Phase 6)"

# Download the newest CI-built FFmpeg shared bundle for this OS into third_party/ffmpeg/ and print FFMPEG_DIR (`--help` for options)
ffmpeg-fetch *args:
    bash scripts/ffmpeg/fetch.sh {{args}}

# cutweave-media against the FFmpeg bundle (feature `ffmpeg`: backend contract, version lock, log routing; docs/dev/ffmpeg.md)
test-ffmpeg *args:
    . scripts/ffmpeg/env.sh && cargo nextest run -p cutweave-media --features ffmpeg {{args}}

# Everything CI checks that this machine can run: the fast gate, Storybook visual regression,
# axe and offline-fonts tests against this OS's baselines, and the golden frames on the software
# adapter. Run before merging to main.
ci-local:
    just check
    just ui-snapshots
    just golden

# Loom model checks for the document actor (slow; run before touching doc-actor concurrency)
loom:
    RUSTFLAGS="--cfg cutweave_loom" cargo test -p cutweave-commands --release --test doc_loom

# The executable specs: the edit-semantics fixtures (WP-2A.01) and the keyframe reference vectors (WP-2C.01)
spec-check:
    cargo xtask check-fixtures
    python docs/spec/keyframes_ref.py
