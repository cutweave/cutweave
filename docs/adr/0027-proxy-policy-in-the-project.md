# ADR-0027: Store the proxy policy in the project and a proxy mode per asset

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers, on the review of WP-2F.01 (before the follow-up that adds the fields)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.2, §7, §17

## Context

WP-2F.01 ([P2 WBS](../wbs/P2-core-editor.md)) makes proxies: smaller intra-frame copies of slow-to-scrub video in the global cache, used by the preview's scrub frames, never by exports (arch §7). Its plan puts the policy in the document, `project.settings.proxy_policy: off | auto | all`, with a per-asset `media.set_proxy_mode{auto | force | off}`, and [settings.md](../reference/settings.md) lists `proxy_policy` as a project setting that WP-2H.08's `project.set_settings` patches, with the app's `media.proxy_policy` preference copied into new projects (WP-2F.03).

Both are project-format changes, which ADR-0006 says need an ADR. The proxy machinery itself (cache store, jobs, `media.generate_proxy`, `media.delete_proxy`, `media.proxy_status`, the `proxies` cache kind, preview selection) is engine and cache state and does not depend on them, so WP-2F.01 shipped it with the policy fixed at `auto` for every project and left these fields for this decision.

## Decision drivers

- A project should carry its editor's intent (an editor who turns proxies off for a project expects that on every machine), while the proxies themselves stay machine-local cache.
- Older builds must open newer documents without losing the fields, and newer builds must open older ones with no migration step (ADR-0006, arch §4.3).
- Every change an editor makes is an undoable command that agents can make too (ADR-0002, ADR-0005); the settings dialog (WP-2H.08) patches the same key.
- No churn in saved files and golden fixtures for projects that keep the default.

## Considered options

1. **Two optional fields with defaults, no schema bump (leading):** `ProjectSettings.proxy_policy` (`off | auto | all`, default `auto`) and `MediaAsset.proxy_mode` (`auto | force | off`, default `auto`), each left out of the file when it is the default (`skip_serializing_if`), read as the default when absent. An older build keeps them in `extra` and writes them back. Set by the undoable `media.set_proxy_policy{policy}` and `media.set_proxy_mode{media[], mode}` (MCP catalog); `project.set_settings` patches `proxy_policy` too.
2. **A schema version bump with a migration** that writes the defaults into every document: explicit, but a permanent migration and fixture for two fields whose absence already means the default, and it would have to chain after ADR-0020's v2.
3. **Keep the policy outside the document** (an engine or app preference only): no format change, but a project would behave differently on each machine and an agent could not see or change the project's intent through the document.

## Decision

**Decided by:** the maintainers' review of WP-2F.01. Pass criteria: the round-trip test (an older document opens with `auto`; a document with the fields opens in an older build and keeps them in `extra`); the two commands are undoable with a parity test; the WP-2F.01 engine reads the policy from the document instead of its fixed `auto` (`cutweave_ops::media::proxy::DEFAULT_POLICY`), and a policy or mode change re-runs the engine's reconcile (queues what is now wanted, never deletes).

If accepted, option 1: the fields, their commands and the reconcile on change land in a follow-up of WP-2F.01 (small: model fields, two `#[command]`s, the document watcher calling `FullProxies::reconcile` when the policy or a mode changes). Semantics: the policy decides which assets get proxies automatically (`auto`: long-GOP H.264 or HEVC larger than 1920 pixels, faster than 60 fps, or 10-bit HEVC); `force` makes one whatever the policy; `off` makes none and the preview does not use an existing one.

**Fallback** (rejected or deferred): proxies stay as shipped in WP-2F.01: `auto` for every project, explicit `media.generate_proxy` / `media.delete_proxy` per asset, and WP-2F.03's app preference as an engine setting (option 3).

## Consequences

**Positive**

- The project carries the editor's choice; agents read and change it like any edit, and undo covers it.
- No migration: absent fields are the defaults, so existing projects and fixtures do not change.

**Negative / costs**

- Two more fields in the format contract that every future reader must keep.
- The engine must react to policy changes (reconcile) without thrashing: an evicted or failed proxy is still made again only on request.

**Follow-ups**

- The WP-2F.01 follow-up above; WP-2F.03 (settings rows, app default copied into new projects); WP-2H.08 (`project.set_settings` patches `proxy_policy`).

## Links

- Architecture: [§4.2](../01-architecture.md), [§7](../01-architecture.md), [§17](../01-architecture.md)
- Work breakdown: [WP-2F.01, WP-2F.03, WP-2H.08](../wbs/P2-core-editor.md); settings: [settings.md](../reference/settings.md)
- Related: [ADR-0006](0006-project-format.md) (format changes need an ADR), [ADR-0020](0020-media-folders.md) (schema v2)
