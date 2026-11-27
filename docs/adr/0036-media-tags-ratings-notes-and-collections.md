# ADR-0036: Tags, ratings and notes on media, and collections in the project

- **Status:** Accepted (product owner, 2026-11-27)
- **Date:** 2026-11-27
- **Deciders:** product owner and Cutweave maintainers (drafted by the Fable work-package session before WP-2F.06 starts)
- **Supersedes:** none (extends the media model of arch §4.2 and ADR-0020)
- **Architecture refs:** `docs/01-architecture.md` §4.2, §4.3, §5.1, §14, §17

## Context

WP-2F.06 ([P2 WBS](../wbs/P2-core-editor.md)) gives the media bin tags, a rating and a note per asset, collections (manual and smart) and a sidebar that shows the folder tree and the collections. Folders are done ([ADR-0020](0020-media-folders.md)): an asset lives in exactly one folder, and ADR-0020 named collections as the many-to-many view. Search by name, kind, length and use exists (`MediaFilter`, WP-1M.12), shared by the engine and the editor.

The package adds fields to the project document and ten commands, so it needs a decision first (AGENTS.md: contracts change only via ADR). Since the WBS text was written, three things changed that the decision has to take into account:

- The project is at schema v3 (ADR-0035), and two ADRs since then added optional fields without a bump (ADR-0027 proxy policy, ADR-0035 still duration). The WBS still says "schema migration and fixture, chained after v2".
- Collect, archive and relink exist (WP-2F.03, WP-2F.04): a copy of a project can leave assets out (`trim_unused`), so anything that names assets by ID has to survive that.
- `MediaFilter` already has `folder` and `recursive`, so "a smart collection, optionally scoped to a folder" needs no second folder field.

## Decision drivers

- An asset has one home (its folder); grouping across folders must not weaken that or the rule that removing media never changes a timeline.
- Every change is an undoable command that agents and the CLI can run, addressing collections by ID (ADR-0002, ADR-0004).
- An older build must never misread a newer document. Where it would only *ignore* a field and keep it, no version bump is needed (the rule ADR-0035 applied: bump for a new media kind, no bump for a new optional setting).
- Queries are deterministic: no wall clock in command or query code (AGENTS.md), so a filter cannot depend on "now".
- Search stays a scan of the document with one predicate in Rust and TypeScript, fast enough for 10,000 assets.
- Tags and notes are text people type: agents must receive them as untrusted text (ADR-0028).

## Considered options

### Where tags, ratings and notes live

1. **Optional fields on the asset (leading):** `MediaAsset.tags`, `rating`, `note`, left out of the file while empty. No tag registry: the set of tags in a project is whatever its assets carry.
2. **A tag entity (`Project.tags` keyed by `TagId`, assets hold IDs):** renaming a tag is one write and tags can carry a colour, but every assignment needs an existence check, an unused tag has to be garbage-collected or shown as empty, and agents would have to look IDs up for what is, to a person, just a word. Rejected for now; option 1 can grow into it (a rename is a batch edit, which `media.rename_tag` can do in one undo step).
3. **Plugin data (`MediaAsset.custom`):** no format change, but no schema, no invariants and no shared filter. Rejected.

### Collections

1. **A collection entity in the document, manual or smart (leading):** `Project.collections` keyed by `CollectionId`; a manual one stores its members, a smart one stores a `MediaFilter` and its membership is computed.
2. **Smart collections only (saved searches), manual grouping through a tag per collection:** one concept fewer, but "add to collection" would silently create tags, and a collection's order of members would be lost. Rejected.
3. **Collections as editor state (`ui-state.json`):** agents could not see or change them, they would not be undoable and collect/archive would lose them. Rejected, as ADR-0020 rejected it for folders.

### Schema version

1. **No bump: every new field is optional and left out while empty (leading).** An older build opens the document for editing and keeps the fields it does not know (`extra`). The one thing it can break is a manual collection's membership, by removing an asset it does not know a collection names; the model therefore tolerates a member that names no asset (see Decision).
2. **Bump to v4 with a no-op migration:** an older build opens the project read-only, so nothing can go stale. It costs a permanent migration and its fixtures, and it makes every project that merely has a tag unusable in an older build. The WBS text assumed this option.

## Decision

Accepted by the product owner on 2026-11-27, as drafted: optional fields with no schema bump, no `imported_within_days`, and the three-way split of WP-2F.06. Pass criteria for the work packages that build it:

- undo identity for every new command, and the registry property tests (invariants after random command sequences) pass with them;
- `media.list` with tags and text over 10,000 synthetic assets answers in under 30 ms p95, and the Rust and TypeScript predicates agree on the extended shared fixture;
- the smart collection "video, longer than 60 s, tag interview, unused" returns exactly the expected set on a fixture, and its membership follows an edit within 100 ms;
- a v3 document without the new fields reads the same as before, a document with them keeps them through an older build's open and save (`extra`), and a manual collection with a member that no longer exists opens, lists and edits without error;
- the F22 user flow extended as the WBS says (tag, filter, save as a smart collection, reopen, the sidebar shows it; drop a clip on a folder in the sidebar).

We will:

1. **Tags, rating, note on the asset.**
   - `MediaAsset.tags: [String]`: each 1 to 64 characters after trimming, no control characters and no comma; at most 64 per asset. Stored as first typed; two tags are the same when they are equal ignoring case and accents (the folding search already uses), and the stored list is sorted by that folded form without repeats.
   - `MediaAsset.rating: 1..=5`, absent for unrated. Whole stars only.
   - `MediaAsset.note: String`, plain text, at most 2,000 characters, absent when empty.
   - All three are left out of the file while empty.
2. **Collections.** `Project.collections: Map<CollectionId, Collection{ id, name, kind, custom }>` with IDs prefixed `col_`; left out of the file while empty.
   - `kind: manual{ media: [MediaId] }` keeps its members in the order they were added, without repeats. `kind: smart{ filter: MediaFilter }` stores a filter; its members are computed when asked, never stored. A smart collection is scoped to a folder through the filter's own `folder` and `recursive`.
   - Names are 1 to 255 characters without control characters, unique ignoring case. At most 1,000 collections; at most 10,000 members in a manual one.
   - **A member that names no asset is allowed in the stored form.** Queries skip it; any command that edits that collection drops it. Removing an asset with this build (`media.remove`, `media.delete_folder{contents: remove}`) takes it out of every manual collection in the same transaction, so undo puts it back. This is what makes option 1 of the schema question safe, and what lets collect and archive leave trimmed assets out of a copy without rewriting collections by hand (they drop the trimmed IDs from the copy's collections).
   - A smart collection's filter may not name a collection (no collection of collections, so no cycles).
3. **The filter.** `MediaFilter` gains `tags` (the asset has all of them), `min_rating` and `collection` (a member of that collection, manual or smart). `text` also matches tags and notes. The doc comments of the new fields stay short: `media_list` is a core MCP tool with a token budget.
   - **`imported_within_days` is not added.** The document stores no import time, and a filter that depends on today's date breaks the no-wall-clock rule for queries and makes a saved smart collection change by itself. If it is wanted later it needs its own decision: an `imported_at` field written by the engine at import, and a filter that takes an explicit date rather than "days ago".
4. **Commands**, all in the `media` namespace, undoable as one step, in the MCP catalog and the CLI:
   - `media.add_tags{ media[], tags[] }`, `media.remove_tags{ media[], tags[] }` (both idempotent; up to 256 assets and 16 tags per call), `media.rename_tag{ from, to }` (every asset that has it, one step), `media.set_rating{ media[], rating | null }`, `media.set_note{ media, note | null }`;
   - `media.create_collection{ name, kind }`, `media.rename_collection`, `media.delete_collection` (never touches assets), `media.add_to_collection{ collection, media[] }`, `media.remove_from_collection`, `media.set_collection_filter{ collection, filter }` (smart only);
   - queries `media.list_collections` (id, name, kind, member count) and `media.list_tags` (each tag with the number of assets that carry it; the editor's tag editor and Filters popover need it, and so do agents). `media.rename_tag` and `media.list_tags` are additions to the WBS list.
   - `media.list` returns each asset's tags and rating; the note only when asked (`include: ["note"]`), to keep pages small.
   - In MCP results, tags, notes and collection names are untrusted text (ADR-0028).
5. **Schema version:** no bump (option 1). The loader needs no migration; the fields are optional.
6. **What stays out of the document:** whether the bin sidebar is shown, its width and which collection is open are editor state (the workspace layout), like every other panel choice.
7. **Work packages.** WP-2F.06 is larger than four agent-days as written, so it splits: **2F.06a** model, commands, filter, queries, parity (engine only); **2F.06b** tag editor, rating stars, note field, the Filters popover's Tags and Rating, and the `bin.card` menu items (Add tag…, Set rating ▸); **2F.06c** collections in the UI ("Save as smart collection", Add to collection ▸), the bin sidebar with the folder tree and the collections, and the F22 flow.

**Fallback** (not taken; kept for the record): the bin keeps folders and the existing search; grouping across folders waits.

## Consequences

**Positive**

- People can mark and find clips across folders (keep, interview, b-roll; four stars and up), and save a search as a collection that stays current.
- Agents get the same model with the same commands ("tag every clip of Day 2 as b-roll", "make a collection of unused interviews").
- No schema bump: a project with tags still opens for editing in a build from before this change, and loses nothing.

**Negative / costs**

- The stored form may hold a collection member that no longer exists, so `Project::validate` cannot demand that members exist; the property tests have to cover "skipped by queries, dropped by the next edit" instead.
- No tag registry: renaming a tag rewrites every asset that has it (one undo step), and a tag with no asset simply stops existing.
- `text` search now reads tags and notes too; the 10,000-asset budget has to hold with them (the fixture grows).
- Collect and archive (WP-2F.04) and relink must carry the new fields: a collected copy keeps tags, ratings, notes and collections, minus trimmed assets.
- A second concept beside folders. The sidebar has to make the difference plain: a folder is where a clip is, a collection is a list it is on.

**Follow-ups**

- WP-2F.06a, b, c as above. Update the WBS text of WP-2F.06 (schema note, `imported_within_days`, the split) when this is accepted.
- Later, each with its own decision if it changes the format: tag colours (would move to a tag entity), an `imported_at` field with a date filter, sequences in collections, sharing collections between projects.

## Links

- Architecture: [§4.2, §4.3, §5.1, §14, §17](../01-architecture.md)
- Work breakdown: [WP-2F.06, WP-1M.12, WP-2F.07, WP-2F.08](../wbs/P2-core-editor.md); context menus: [09-context-menus.md](../research/09-context-menus.md)
- Related: [ADR-0020](0020-media-folders.md) (one folder per asset; collections as the many-to-many view), [ADR-0027](0027-proxy-policy-in-the-project.md) and [ADR-0035](0035-still-duration-and-image-sequences-in-the-project.md) (optional fields without a schema bump; when a bump is needed), [ADR-0028](0028-mcp-result-schemas.md) (untrusted text in MCP results), [ADR-0006](0006-project-format.md)
