# ADR-0042: An export warning code for a transparent background that the file cannot carry

- **Status:** Accepted (product owner, 2026-11-28); built under issue #238
- **Date:** 2026-11-28
- **Deciders:** Product owner (issue #238)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §7 (export), §14 (commands and errors); WP-2B.02 (background fill)

## Context

WP-2B.02 added a transparent background (`format.background_fill` of kind `transparent`). The WBS says it is "only for alpha-capable exports". Exporting such a sequence to a file without an alpha channel gives black where the picture is transparent, and nothing says so (#238).

The export commands already answer with a list of warnings: `export.resolve_preset`, `export.start` and `export.enqueue` return `warnings`, each with a code from `PresetWarningCode`. The Export dialog shows them in its notes list, and agents and the CLI read the same list. The engine also already knows whether a planned export carries alpha (`PlanOutput.alpha`: today ProRes with a `yuva…` pixel format).

Telling people needs one new value in that code list. That is a change to the command schema (the schema hash changes), so it needs a decision, although no field is added.

## Decision drivers

- A person and an agent should learn about the black background before the export runs, through the same channel.
- The editor should hold no rule about which codecs carry alpha; the engine knows.
- The smallest change to the contract.

## Considered options

1. **A new warning code for a transparent background** (`TransparentBackground` on the wire) in the existing `warnings` of the three export commands, set by the engine when the sequence's background is transparent and the planned output has no alpha. The export is not blocked.
2. **A rule in the editor only.** No schema change, but agents and the CLI get nothing, and the editor would need its own list of alpha-capable codecs.
3. **Refuse the export.** Stricter than the WBS asks, and it blocks a person who wants the black background.

## Decision

**Option 1.** `PresetWarningCode` gains one code for a transparent background that the file cannot carry. On the wire it is written `TransparentBackground`, like the other codes of that list (`AspectMismatch`, `SoftwareEncoder`). The engine sets it in the `warnings` of `export.resolve_preset`, `export.start` and `export.enqueue` when the sequence's background is transparent and the planned output has no alpha channel. The export is not blocked.

**Decided by:** the product owner, 2026-11-28.

**Pass criteria:** the three export commands return the code when, and only when, the background is transparent and the planned output has no alpha; the Export dialog shows it with a translated sentence and leaves Export enabled; an export started by an agent or the CLI carries the same warning; flow F67 covers the dialog.

## Consequences

**Positive**

- One rule, in the engine, for every caller.

**Negative / costs**

- A client that matches warning codes exhaustively sees a code it does not know. The list is already open-ended in practice (codes were added in Phase 2).
- The dialog's check before the export does not see an unsaved pixel-format change in the advanced options, because `export.resolve_preset` takes no advanced options. The answer of `export.start` is exact. Closing that gap would need a new field on the query and is not part of this decision.

**Follow-ups**

- No built-in preset carries alpha today (only ProRes 422 HQ exists). An alpha-capable preset, such as ProRes 4444, is a separate piece of export work.

## Links

- Issue: #238
- Work packages: `../wbs/P2-core-editor.md` (WP-2B.02, WP-2G.04)
- Related: [ADR-0030](0030-export-queue-in-the-project.md), [ADR-0038](0038-machine-error-code-on-failed-jobs.md)
