# Governance

This is a stub. It describes how decisions are made today, while the project is small. A fuller version arrives with the documentation work in Phase 7 (epic 7E).

## Maintainers

Maintainers are the members of the `@cutweave/maintainers` GitHub team. They review and merge PRs, accept ADRs and RFCs, and handle security reports and Code of Conduct reports.

The team's members are listed on the GitHub organization page. A maintainers table will be added here when the team is established.

## Decision process

- **Day-to-day changes** go through pull requests following [CONTRIBUTING.md](CONTRIBUTING.md). One maintainer approval plus green CI merges.
- **Settled product decisions** live in [docs/00-product-brief.md](docs/00-product-brief.md) and are not reopened in PRs.
- **Architecture-level changes** (command schemas, WIT interfaces, the project file format, the plugin manifest, the MCP surface, crate boundaries) need an **ADR** in `docs/adr/` (MADR format: context, options, decision, consequences). The contract is [docs/01-architecture.md](docs/01-architecture.md). Only maintainers accept ADRs. See [docs/06-dev-process.md](docs/06-dev-process.md) §9.
- **Larger user-visible or ecosystem changes** (plugin API, project format, MCP surface) go through an **RFC** in `docs/rfc/`, with a two-week comment window once external contributors exist. The full RFC process is still to be written.
- Disagreements are settled by maintainer consensus; if there is none, the tech lead decides and records the reasoning in the ADR.

## Roles

- **Tech lead:** reviews changes to contracts (see `.github/CODEOWNERS`).
- **Release owner:** cuts releases and owns the release checklist (see [docs/06-dev-process.md](docs/06-dev-process.md) §11).
- **Design-system owner:** owns `design/` and `packages/ui`: the token set in `design/tokens.json`, component specs and the voice and states rules in `design/README.md`. UI changes use the design system only. A contributor who needs a missing value adds a token with a usage note in the same PR, and the design-system owner reviews it. Changes that alter the visual language or existing tokens need the owner's approval.
- **Legal reviewer:** reviews licensing-sensitive changes (`LICENSE*`, `wit/`, `packages/plugin-sdk`) and the plugin exception.

## Changes to this document

Changes to governance are made by PR and need approval from a majority of maintainers.
