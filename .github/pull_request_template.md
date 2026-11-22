<!-- Branch name: wp/<WP-id>-<slug>. See CONTRIBUTING.md and docs/06-dev-process.md §3. -->

## Work package

- **WP id:** WP-
- **Linked issue:** Closes #
- **Architecture sections touched:** <!-- e.g. §5.1, §15 (docs/01-architecture.md) -->

## Summary

<!-- What changed and why, in a few lines. -->

## Acceptance criteria

<!-- Copy the criteria from the WP. Check each one and link evidence:
     test names, benchmark numbers, screenshots, golden diffs. -->

- [ ] Criterion 1: evidence
- [ ] Criterion 2: evidence

## Definition of Done (docs/06-dev-process.md §6)

- [ ] `just check` is green; WP-specific gates are green
- [ ] Tests added at the level the WP specifies
- [ ] Every new mutation is a registered command (schema, TS client and MCP catalog updated; parity test passes)
- [ ] No secrets or paths in logs; filesystem and network access go through allowlists and the gateway
- [ ] Docs updated; schema snapshots updated if a contract changed
- [ ] License headers present; no new dependency fails `cargo-deny` / the JS license check (justify any new dependency below)

## ADR needed?

<!-- Link the ADR/RFC (required if you changed a command schema, WIT, the project format,
     the plugin manifest or the MCP surface), or write "No: reason". -->

## Notes for integrator

<!-- Merge-order hints, follow-ups, deviations from the WP, files that may conflict with other in-flight WPs. -->

## Contribution checklist

- [ ] Every commit is signed off (`git commit -s`), per the [Developer Certificate of Origin](https://developercertificate.org/). The DCO check blocks the PR otherwise.
- [ ] AI-assisted: yes / no. If yes, a human reviewed every line, and AI-authored commits carry a `Co-Authored-By` trailer (see CONTRIBUTING.md).
