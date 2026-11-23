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

# Render golden-frame projects headless and compare (`just golden --update`)
golden *args:
    @echo "golden: not implemented yet (arrives in WP-0I)"

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

# Start the headless engine and run the MCP conformance/inspector suite
mcp-inspect:
    @echo "mcp-inspect: not implemented yet (arrives in WP-1N)"

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

# Everything CI checks that this machine can run: the fast gate plus Storybook visual regression,
# axe and offline-fonts tests against this OS's baselines. Run before merging to main.
ci-local:
    just check
    just ui-snapshots

# Loom model checks for the document actor (slow; run before touching doc-actor concurrency)
loom:
    RUSTFLAGS="--cfg cutweave_loom" cargo test -p cutweave-commands --release --test doc_loom
