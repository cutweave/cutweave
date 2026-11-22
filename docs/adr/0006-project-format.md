# ADR-0006: Project format is a folder with a JSON document, an append-only journal and rotating backups

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Cutweave maintainers (product owner approved)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.3 (schema versioning), §17, §7 (caches)

## Context

Brief principles 6 and 8 require that user work is never lost, and that project files are in an open, documented format with no account. Projects are edited by agents as well as humans, and are shared and sometimes versioned in git. Research 03 §5.2 weighed a single-file SQLite project against a zip or directory with JSON.

## Decision drivers

- Crash safety: atomic saves, journal replay, backups.
- Human-diffable, mergeable, documented (open format).
- Rebuildable caches kept separate from irreplaceable user data.
- Forward migrations; older apps must not corrupt newer files.
- Portability ("collect and archive") and media relinking.

## Considered options

1. **Project folder: `<Name>.cutweave` JSON document + `generated/` + `.cutweave/{journal,backups,lock}`** (chosen).
2. **Single-file SQLite project.** Rejected for the document: atomic and incremental, but not diffable or mergeable, and opaque to agents and tools. SQLite is kept for rebuildable caches and the job queue (`cache.sqlite`).
3. **Zip container (project.json + thumbnails).** Rejected as the primary form: every save rewrites the archive, and recovery and diffing are harder. A portable archive is produced by "collect and archive" instead.
4. **Binary or MessagePack document.** Rejected: not human-inspectable; no benefit that outweighs the loss of diffability.

## Decision

A project is a user-chosen folder with the JSON document `<Name>.cutweave` (the file users open, carrying `schema_version`), a `generated/` folder for media created by Cutweave, a `compositions/` folder, and a `.cutweave/` directory holding the append-only command `journal/`, rotating `backups/` and a single-writer `lock`. Saves are atomic (temp file, fsync, rename). Autosave runs on idle and every 30 s when dirty, and persists only the last committed revision (ADR-0017). Media is referenced by absolute path, relative path and content hash; relinking matches by hash. Global caches (proxies, thumbnails, waveforms, frame index) live outside the project and are rebuildable. `ui-state.json` holds non-undoable `view` state. Ordered `vN → vN+1` migrations with golden fixtures are mandatory; files from newer app versions open read-only with a warning. Changing this format is a contract change that needs an ADR (and, per the dev process, an RFC).

## Consequences

**Positive**

- Crash recovery is replay of the journal over the last save, with a restore/discard UI.
- The JSON document is diffable, inspectable by agents, and git-friendly.
- Irreplaceable data (document, generated media) is separated from disposable caches.

**Negative / costs**

- Folders are less convenient to email than one file; "collect and archive" and an interchange export (OTIO) cover that.
- Large JSON documents need careful (streaming, bounded) load; the open-1-hour-400-item budget (< 2 s, arch §19) guards it.
- A single-writer lock file has to cope with stale locks (pid and host recorded).
- Migrations are a permanent maintenance obligation.

**Follow-ups**

- Epic 1C (persistence and recovery), fuzzing of project load (arch §20), interchange in `cutweave-interchange`.

## Links

- Architecture: [§4.3](../01-architecture.md), [§17](../01-architecture.md), [§7](../01-architecture.md)
- Research: [03-tech-stack.md](../research/03-tech-stack.md) §5.2 (project file), §5.4 (cache and proxies)
- Related: ADR-0005, ADR-0017, ADR-0015
