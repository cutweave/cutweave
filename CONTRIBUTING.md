# Contributing to Cutweave

Thanks for helping. Cutweave is a GPL-3.0 desktop video editor with a Rust engine, an Electron + React UI and full MCP control. Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first.

## Where things are

- [docs/00-product-brief.md](docs/00-product-brief.md): settled product decisions
- [docs/01-architecture.md](docs/01-architecture.md): the architecture contract (cite § numbers in PRs)
- [docs/06-dev-process.md](docs/06-dev-process.md): environment, conventions, Definition of Done, review checklist
- [docs/wbs/](docs/wbs/): the work packages (WPs)
- [AGENTS.md](AGENTS.md): the non-negotiable rules for anyone (human or AI) writing code
- [design/README.md](design/README.md): the design system
- [GOVERNANCE.md](GOVERNANCE.md) and [SECURITY.md](SECURITY.md)

## Setting up

Per-OS prerequisites (Rust toolchain, Node, pnpm, FFmpeg) are in [docs/06-dev-process.md](docs/06-dev-process.md) §2. Then, from a clone:

```sh
just setup    # toolchains, pnpm install, FFmpeg bundle, git hooks, corpus manifest
just dev      # engine + Electron with HMR
just check    # everything CI runs on a PR; must pass before you open one
```

None of that needs FFmpeg: only `cutweave-media`'s `ffmpeg` feature links it. To work on the media backend, set up the FFmpeg bundle and run `just test-ffmpeg` as described in [docs/dev/ffmpeg.md](docs/dev/ffmpeg.md).

## Workflow

1. Pick (or create) an issue. Planned work is one issue per work package, generated from `docs/wbs/`. Read the WP's scope and acceptance criteria; if it is ambiguous or larger than about 4 agent-days, say so on the issue instead of guessing.
2. Branch from `main` as `wp/<WP-id>-<slug>`, for example `wp/1B.04-undo-property-tests`. For work outside a WP use a short descriptive slug (`fix/<slug>`, `docs/<slug>`).
3. Write tests first from the acceptance criteria. Keep to the rules in [AGENTS.md](AGENTS.md): every mutation is a registered command, time is flicks, entities are addressed by ID, UI uses design-system tokens only, no new dependency without justification.
4. Run `just check`.
5. Open a PR using the template: WP id, acceptance-criteria checklist with evidence, architecture sections touched and "Notes for integrator". PRs are squash-merged.

Changes to a command schema, a WIT interface, the project file format, the plugin manifest or the MCP surface need an ADR (`docs/adr/`) or RFC first. See [GOVERNANCE.md](GOVERNANCE.md).

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, for example `feat(timeline): ripple delete across tracks` or `fix(export): keep audio in sync after trim`. Common types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`.

## Sign-off (DCO)

Cutweave uses the [Developer Certificate of Origin](https://developercertificate.org/) instead of a CLA. Every commit must carry a `Signed-off-by:` line, which certifies that you wrote the change or have the right to submit it under the license indicated in each file you change (see [Licensing](#licensing)):

```sh
git commit -s -m "feat(model): add clip speed field"
```

Forgot? Fix a branch with `git rebase --signoff origin/main` and push with `--force-with-lease`. A GitHub Actions check blocks PRs containing unsigned commits.

## AI-assisted and AI-agent contributions

Much of Cutweave is built with AI agents, and contributions that use them are welcome under these rules:

- **Disclose it.** Tick the AI-assisted line in the PR template. AI-authored commits carry a `Co-Authored-By:` trailer naming the model, in addition to the human or operator's `Signed-off-by:`.
- **A human is accountable.** The person who opens the PR (or the operator who ran the agent) signs off, has reviewed every line, and can explain and maintain it. Agent output is not trusted because it is confident.
- **Same bar as any code.** Agents follow [AGENTS.md](AGENTS.md), work in their own worktree and branch, and must pass `just check` and the review checklist in [docs/06-dev-process.md](docs/06-dev-process.md) §7.
- **No secrets, no tainted code.** Never paste API keys, tokens or private data into prompts or commits. Do not submit code copied from sources whose license is incompatible with GPL-3.0, including model output you know reproduces such code.
- Maintainers may close PRs that are unreviewed bulk agent output.

## Licensing

The core is **GPL-3.0-or-later** (see [LICENSE](LICENSE)). The WIT interfaces, the plugin SDK and the composition runtime (`wit/`, `packages/plugin-sdk`, `packages/compose-runtime`) are **MIT OR Apache-2.0**, with the license texts in each directory. Every source file carries an SPDX header that says which applies; files without one are covered by [REUSE.toml](REUSE.toml). A **plugin exception** is being drafted so that plugins using only Cutweave's published interfaces may use any license; see [LICENSE-PLUGIN-EXCEPTION.md](LICENSE-PLUGIN-EXCEPTION.md). It is a draft pending legal review and is not yet in effect. None of these licenses grants rights to the Cutweave name or logo.

**Your sign-off and the license in the file.** Your `Signed-off-by:` certifies, under the DCO, that you may submit your change under the license indicated in each file you change, which is its SPDX header (or `REUSE.toml` for files without one). Do not change a file's license header unless a maintainer asks you to.

**The permissive directories.** Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in `wit/`, `packages/plugin-sdk` or `packages/compose-runtime`, as defined in the Apache-2.0 license, is licensed as above (MIT OR Apache-2.0), without any additional terms or conditions. Code may move from those directories into GPL-licensed parts of Cutweave (keep its notices), but not the other way: do not copy GPL-licensed code, or code generated from it, into them unless every author of that code agrees in writing.

**The plugin exception (pending legal review).** An additional permission such as the plugin exception covers only code whose authors agreed to it. By signing off a contribution to a GPL-licensed part of Cutweave, you also agree that it may be distributed under GPL-3.0-or-later together with the plugin exception in LICENSE-PLUGIN-EXCEPTION.md, as adopted by the maintainers after legal review. Any adopted version may only add permissions for works that use Cutweave's published interfaces; it never removes rights the GPL gives. The maintainers do not accept contributions to GPL-licensed parts of Cutweave on other terms, and will replace this paragraph with counsel-reviewed wording, and adopt the exception, before the first external contribution is merged. The reasoning is in [docs/legal/first-pass-review-2026-11-23.md](docs/legal/first-pass-review-2026-11-23.md).

New dependencies must be GPL-3.0-compatible and pass `cargo-deny` and the JS license check (policy: [docs/01-architecture.md](docs/01-architecture.md) §21).

## Reporting bugs and security issues

Use the issue templates for bugs and feature requests. For vulnerabilities, follow [SECURITY.md](SECURITY.md) and do not file a public issue.
