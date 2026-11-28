# ADR-0030: Keep the export queue in the project folder (`.cutweave/exports.sqlite`)

- **Status:** Accepted (product owner, 2026-11-26)
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers, product owner (on the review of WP-2G.03a)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §7, §17

## Context

WP-2G.03a ([P2 WBS](../wbs/P2-core-editor.md)) makes exports a background queue that "survives engine restart": queued exports run after a crash or a restart, and one that was running starts again from the beginning. Each queued export renders the committed document of the moment it was queued, so the queue has to keep that document, not only the request.

ADR-0006 fixes the project folder as the document, `generated/`, `compositions/` and `.cutweave/{journal,backups,lock}` (plus `ui-state.json`), and says changing it needs an ADR. Arch §17 lists the same files and does not reserve `.cutweave/` for any engine state. Keeping the queue there adds a file to that layout, so it needs this ADR. The WBS already decided SQLite through `cutweave-jobs`, and the integrator decided where the queue lives on 2026-11-25. This ADR records those choices for the owner to accept.

## Decision drivers

- Only one engine may run a queue, or two engines would export the same job. The project's single-writer lock (`.cutweave/lock`) already guarantees one writer per project.
- A queued export is self-contained (a document snapshot), but the person thinks of it as part of the project they were editing.
- No data loss on a crash: an acknowledged `export.enqueue` must be on disk.
- An untitled document has no folder; its queue must still work.
- Deleting the global caches (`--cache-dir`) must stay harmless (arch §7: caches are rebuildable).

## Considered options

1. **Per project, `<project>/.cutweave/exports.sqlite` (chosen):** jobs, their order, the pause and each job's payload (the resolved plan and the committed document in the project format) in a SQLite file next to the journal. An untitled document's queue lives in memory and moves into the project it is saved as (or the next one opened). Switching to another project suspends the old project's exports: a running one stops and runs again from the beginning when that project is reopened.
2. **Machine-wide, next to `cache.sqlite`:** one queue for the computer, running across project switches. Several engines (the app and headless agent engines) share the cache folder, so this needs a lease between engines. It also puts non-rebuildable state in a folder people are told they may delete.
3. **In the project document:** queued exports as document state. Exports are not edits: they would enter undo, the journal and every agent's view of the document, and each snapshot would grow the document.

## Decision

Accepted by the product owner on 2026-11-26 as built in WP-2G.03a (option 1).

**Decided by:** the owner's review of WP-2G.03a. Pass criteria are its acceptance tests: three queued exports, the engine killed during the second one and restarted, all three files valid (ffprobe), and no partial file left (`export_queue_ffmpeg.rs`). Also: a queue that survives a clean restart, stays with its project on a switch, and moves along when an untitled document is saved (`export_queue.rs`).

Option 1, as built in WP-2G.03a:

- **File:** `.cutweave/exports.sqlite`, append-only migrations counted by `PRAGMA user_version` (schema 1: `export_jobs`, `queue_meta`; schema 2 adds the nullable `export_jobs.error_code`, ADR-0038), WAL journal with `synchronous = NORMAL`. A file from a newer build is refused, and the queue then runs in memory and logs a warning. A read-only project (newer schema) also keeps its queue in memory.
- **Payload:** the resolved plan plus the committed document as project-format JSON (`StoredPlan`, version 1), migrated on load like a project file. A payload that cannot be read is dropped from the queue, with a log line.
- **Engine-private:** like the journal, the file is not part of the document contract. Clients reach it only through the `export.*` commands. It is never written into backups or `generated/`, and the document never references it.

**Fallback** (rejected or deferred): the queue keeps the same commands but lives only in memory, so queued exports are lost when the engine stops, and the restart criterion is dropped from WP-2G.03a.

## Consequences

**Positive**

- One engine runs a project's queue, because the project lock already ensures it; no new lease is needed.
- A killed engine loses no queued export, and a restart resumes the queue.
- The cache folder stays disposable.

**Negative / costs**

- A switch to another project pauses the old project's exports until it is reopened (owner decision 2, 2026-11-25).
- Each queued export stores a copy of the document. That is about the size of the `.cutweave` file per job, until the job is removed from the queue.
- Another file in the project folder for collect-and-archive (WP-2F.04) and backups to leave out.

**Follow-ups**

- WP-2G.04 (queue panel) shows the queue and its pause, and says that switching projects suspends exports.
- WP-2F.04 (collect & archive) leaves `.cutweave/exports.sqlite` out.

## Links

- Architecture: [§7](../01-architecture.md), [§17](../01-architecture.md)
- Work breakdown: [WP-2G.03a, WP-2G.03b, WP-2G.04](../wbs/P2-core-editor.md)
- Related: [ADR-0006](0006-project-format.md) (project folder), [ADR-0015](0015-wire-protocol.md) (deterministic ids)
