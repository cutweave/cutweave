# ADR-0020: Media folders: a folder tree in the project, one folder per asset

- **Status:** Proposed
- **Date:** 2026-11-24
- **Deciders:** product owner and Cutweave maintainers (drafted by the planner for the integrator)
- **Supersedes:** none (extends the media model of arch §4.2; narrows the "folders/bins" exclusion of WP-1M.03 and WP-2F.06)
- **Architecture refs:** `docs/01-architecture.md` §4.2, §4.3, §5.1, §14, §17

## Context

The media bin (WP-1M.03) is a flat list of every asset in the project, ordered by import. A YouTube edit easily holds a few hundred clips: interviews, B-roll, music, sound effects, graphics. The product owner asked for folders in the Assets panel (2026-11-24). The roadmap had only "media bin search, tags and smart collections" (WP-2F.06), which explicitly excluded "folders/bins hierarchy beyond collections".

Folders change the project format and add commands, so they need an ADR (AGENTS.md: contracts change only via ADR). They also have to fit what is already decided: entities are addressed by ID, never by index or path (ADR-0004, arch §4.3); every change is an undoable command that agents and the CLI can run (ADR-0002); and saved projects migrate forward (arch §4.3, schema versioning).

What other editors do (research for the planning report, November 2026): Premiere Pro, DaVinci Resolve and Final Cut Pro all give each item exactly one home (a bin or an event) and offer many-to-many views only as smart or keyword collections. Premiere (since 23.6) and Resolve mirror an OS folder's structure on import; Premiere users had complained for years when it flattened folders. Deleting a bin whose clips are used in a timeline is handled with a warning (Premiere removes the timeline uses, Resolve leaves them offline). CapCut desktop has no user folders in its Media panel.

## Decision drivers

- People expect a clip to live in one place, and moving it to be a move, not a copy.
- Agents must be able to see and change the organization with the same commands as the UI, addressing folders by ID.
- Every operation is undoable as one step, and none of them may damage a timeline or touch files on disk.
- Many-to-many grouping is still needed (a clip that is both "interview" and "best takes"); WP-2F.06's tags and collections provide it.
- A small, invariant-checked model that property tests can cover, with a clean migration from schema v1.

## Considered options

1. **A folder tree entity in the document, one folder per asset** (chosen): `Project.folders` keyed by `FolderId`, each with a parent; `MediaAsset.folder` names its folder or none (top level).
2. **Many-to-many membership (folders as labels, like Final Cut Pro's keyword collections).** Rejected as the folder model: "move" and "delete folder" become ambiguous (does deleting remove the clip from the project or only from this label?), and it duplicates WP-2F.06's manual collections, which already give many-to-many views.
3. **A folder path string on each asset (`folder: "Interviews/Day 1"`).** Rejected: renaming a folder rewrites every asset in it, empty folders cannot exist, and it addresses by path, not by ID.
4. **Folders as UI state only (`ui-state.json` or the editor's local storage).** Rejected: agents could not see or change them, they would not be undoable, and they would be lost by collect and archive (WP-2F.04).

## Decision

**Decided by:** the product owner and the maintainers, before WP-2F.07 starts. Pass criteria for building on it: WP-2F.07's registry property tests (undo identity, invariants after random command sequences), the v1→v2 migration goldens, and the F22 user flow (WP-2F.08). Until then this is a proposal and nothing builds on it.

We will add a folder tree to the project document:

- **Model.** `Project.folders: Map<FolderId, MediaFolder{id, name, parent?: FolderId, custom}>` with IDs prefixed `fld_`. `MediaAsset.folder?: FolderId` (absent = the top level) and `MediaAsset.name?: String`, a display name for the bin (absent = the file name), so assets can be renamed while they are organized.
- **Invariants** (checked by `Project::validate` and property-tested): every parent and every asset's folder exists; there are no cycles; nesting is at most 32 levels; folder names are 1 to 255 characters with no `/` or control characters, and unique among siblings ignoring case. Unique names let people and agents refer to a folder by its path without ambiguity.
- **One home.** An asset is in exactly one folder or at the top level. Many-to-many grouping is WP-2F.06's manual collections and its saved searches (smart collections), which may be scoped to a folder.
- **Commands** in the existing `media` namespace (no new namespace), all undoable as one step and reachable over MCP (catalog) and the CLI: `media.create_folder`, `media.rename_folder`, `media.rename` (asset display name), `media.move` (assets and folders, to a folder or the top level, `on_conflict: fail | rename`), `media.delete_folder{contents: move_up | remove}`. `media.import` and `media.register` take an optional target folder; importing an OS folder may recreate its structure (WP-2F.09).
- **Deleting a folder never changes a timeline and never touches files on disk.** `move_up` lifts its contents one level. `remove` removes the folder, its subfolders and every asset no timeline uses; assets that are used are kept and moved to the deleted folder's parent, and the result says which. To remove those too, a person or agent deletes their clips first, as `media.remove` already requires.
- **Queries.** `media.list` filters by folder (one level or recursive) and returns each asset's `folder` and `name`; `media.list_folders` returns the tree with paths and counts.
- **Out of the model for now:** sequences in folders (sequences keep their own tabs, WP-2B.03), a manual order inside a folder (the bin sorts), and folder colors. The model can grow to cover them later without changing these rules.
- **Migration.** Schema v1 → v2 adds `folders: {}` and `folder: null` and `name: null` on every asset. As for every newer schema, older apps open v2 projects read-only.

## Consequences

**Positive**

- People organize a large project the way every NLE trains them to, and agents can do the same thing ("put the drone shots in B-roll/Drone") with the same commands.
- Folders stay simple: one home per asset, deletes that cannot break a timeline, everything undoable.
- Search (WP-1M.12) and smart collections (WP-2F.06) work across folders or within one, on the same filter.

**Negative / costs**

- A schema migration and new golden fixtures. If other P2 work packages also bump the schema (WP-2F.06 tags, WP-2A.02 markers), the integrator orders their migrations at merge.
- Name uniqueness among siblings means a move or create can clash; the commands either fail or rename ("Day 1 2"), and the UI always asks for the rename.
- A person who wants a clip in two places uses a collection, which is a second concept to learn.

**Follow-ups**

- WP-2F.07 (model, commands, migration), WP-2F.08 (bin UI), WP-2F.09 (import folders with their structure); WP-2F.06 is re-scoped to tags, ratings, notes, collections and the bin sidebar's folder tree.
- Arch §4.2 (entities: `folders`, `MediaAsset.folder`, `MediaAsset.name`; ID prefix `fld_`) and §23 (decision log) once accepted.

## Links

- Architecture: [§4.2](../01-architecture.md), [§4.3](../01-architecture.md), [§5.1](../01-architecture.md), [§14](../01-architecture.md), [§17](../01-architecture.md)
- Work packages: [WP-2F.06 to WP-2F.09](../wbs/P2-core-editor.md), [WP-1M.12](../wbs/P1-skeleton.md)
- Related ADRs: ADR-0002, ADR-0004, ADR-0005, ADR-0006
