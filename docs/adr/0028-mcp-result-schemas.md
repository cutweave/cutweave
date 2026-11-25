# ADR-0028: Described output schemas describe the MCP result

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (drafted by the issue #146 fix)
- **Supersedes:** none (amends the MCP surface of ADR-009, arch §14 and §23)
- **Architecture refs:** `docs/01-architecture.md` §14, §18, §23

## Context

`cutweave_describe_command` returned the registry's output schema, the one the RPC layer's results satisfy. MCP results differ from the RPC output in two documented ways:

- Text that comes from media files is wrapped as `{"untrusted": true, "text": "..."}` (prompt-injection hygiene, §14, §18). The fields are listed in `results::UNTRUSTED_FIELDS`: progress messages and errors of `job_wait`, `job_status`, `job_list` and `job_cancel`, and `media_import` failures.
- `render_frame` returns the picture as an image content block, so `structuredContent` has no `data`.

A client that validates `structuredContent` against the described schema rejected real results, for example every `job_wait` on an analysis job. The M1 exit suite found it (WP-1O.04, "Found by the suite" item 1, issue #146) and the conformance checker worked around it by unwrapping the wrappers before validating.

## Decision drivers

- A described schema must be true of what the tool returns; a client may validate against it.
- The shape rules must live in one place, so the schema and the result cannot drift apart.
- The tools list must stay within the R33 budget (at most 20 tools, about 5k tokens).
- RPC and the CLI keep their results and schemas unchanged.

## Considered options

1. **Describe the MCP shape, generated from the wrapping tables** (chosen). `cutweave_describe_command` returns `output_schema` as MCP returns it, derived from `UNTRUSTED_FIELDS` and the image field table by the same code path that applies them to results.
2. **Serve both schemas** (`output_schema` for RPC, `mcp_output_schema` for MCP). Rejected: two schemas to read, and a client would pick the wrong one.
3. **Drop the wrappers** over MCP. Rejected: they are the untrusted-text defence of §14.
4. **Keep the registry schema and document the differences.** Rejected: it is the bug.

## Decision

`output_schema` in `cutweave_describe_command` is the schema of the MCP result's `structuredContent`:

- Each untrusted field's string schema becomes the wrapper's schema: `{"type":"object","properties":{"untrusted":{"const":true},"text":{"type":"string"}},"required":["untrusted","text"],"additionalProperties":false}`. A nullable field stays nullable (`anyOf` of `null` and the wrapper), and its description is kept.
- `render_frame`'s `data` is removed from `properties` and `required`. The picture is the image content block (or, above 1568 px, a resource link); `mime` and the metadata stay in `structuredContent`.
- Other commands' schemas are unchanged.

The schema is built by `results::mcp_output_schema` from `UNTRUSTED_FIELDS` and `IMAGE_FIELDS`, the tables `wrap_untrusted` and `render_frame` use. Adding a field to a table changes both the result and its schema. The input schema, the RPC and CLI results and the registry are unchanged. `outputSchema` is still not listed in `tools/list`.

**Decided by:** the conformance suite validates every result against the described schema with no unwrapping (the checker's `unwrap_untrusted` is removed), plus a check that the described `job_wait` error accepts the wrapper and `null` and refuses a plain string. Fallback: revert to the registry schema and the unwrapping checker.

## Consequences

**Positive**

- A client can validate `structuredContent` against the described schema.
- One table drives results and schemas; the conformance suite catches a drift.
- No change to the tools list: 20 tools, about 4908 tokens before and after (Standard profile, full engine); 19 tools, about 4589 tokens on the stub engine.

**Negative / costs**

- `output_schema` from `cutweave_describe_command` no longer equals the RPC one for 5 commands (`media.import`, `job.status`, `job.wait`, `job.list`, `job.cancel`) plus `render.frame`. A client that read the old schema to decode a described `job_wait` as a plain string must read `error.text` and `progress.message.text` (it already had to, since the results were wrapped).
- The parity test still compares MCP with RPC after unwrapping, by design.

**Follow-ups**

- Transcripts and media metadata join `UNTRUSTED_FIELDS` with the commands that return them; their schemas follow without more work.

## Links

- Architecture: `../01-architecture.md` §14, §18
- Issue #146; [M1 exit](../milestones/M1-exit.md) "Found by the suite" item 1
