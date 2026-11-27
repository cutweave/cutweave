# ADR-0038: A failed job carries a machine error code beside its message

- **Status:** Accepted (product owner, 2026-11-27); built under issue #226
- **Date:** 2026-11-27
- **Deciders:** product owner (issue #202)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §7 (export), §14 (commands and errors); ADR-0015 (wire protocol); ADR-0030 (export queue)

## Context

WP-2G.04 asks the export queue panel to show a failed export's "machine code and human message". The panel shows the message. There is no code to show: a job fails with `JobError::Failed(String)`, and `JobInfo.error` and `ExportQueueJob.error` are that sentence and nothing else (`crates/cutweave-jobs/src/types.rs`, `crates/cutweave-engine/src/export/pipeline.rs`). The export pipeline builds the sentence from many sources (the encoder, the renderer, the disk, missing media), so the editor cannot derive a stable code from the text without guessing, and an agent that retries a failed export has the same problem.

Adding a field to `JobInfo` and `ExportQueueJob` changes the command schemas and the `job.progress` event, which is a contract change (AGENTS.md: contracts change only via ADR).

## Considered options

1. **An optional `error_code` on `JobInfo` and `ExportQueueJob`** (proposed): a short stable identifier beside the message.
2. Prefix the message with a code (`DiskFull: …`): no schema change, but every reader has to parse text, and the existing messages would change under agents that read them.
3. Leave it: the row shows only the message.

## Decision

Accepted by the product owner on 2026-11-27 as drafted: an optional `error_code` on `JobInfo` and `ExportQueueJob`, the export codes below, stored with the queued job and shown on the queue row.

A failed job carries `error_code: string | null` next to `error`, on `JobInfo` (so `job.status`, `job.list`, `job.wait` and the `job.progress` event) and on `ExportQueueJob` (`export.queue` and the queue commands). It is optional and absent on old data, so no schema version changes.

- `JobError::Failed` gains a code; `JobError::failed(message)` keeps working and means `failed`, the general code.
- The codes are lower snake case and few. Export starts with: `disk_full`, `output_exists`, `output_unwritable`, `media_offline`, `encoder_failed`, `render_failed`, `engine_unavailable`, and `failed` for the rest. A cancelled job has no code.
- The export queue stores the code with the job (ADR-0030's SQLite table gains a nullable column), so it survives a restart.
- The queue panel shows the code in mono before the message; "Copy error" copies both. The OS notification and the toast keep the message only.

## Consequences

- An implementation work package follows: `cutweave-jobs` (the error type and `JobInfo`), the export pipeline's failure sites, the queue store's column, the generated schema and TS types, the queue row and its tests, and the F65 flow's failed row.
- Other job kinds (analysis, proxy, collect, archive) report `failed` until their own work packages name codes.
- Until #226 is built, a failed export row shows the engine's message only.
