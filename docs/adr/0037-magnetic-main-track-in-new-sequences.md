# ADR-0037: Let the caller say whether a new sequence's main track is magnetic

- **Status:** Accepted (product owner, 2026-11-27)
- **Date:** 2026-11-27
- **Deciders:** Product owner
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §5.1 (commands), §16 (preferences); [ADR-0023](0023-timeline-edit-semantics.md) (the magnetic main track)

## Context

WP-2A.13 plans the Timeline preference "Main track is magnetic in new sequences" (`timeline.magnetic_main`, default on; [settings catalog](../reference/settings.md)). The work package says the value is "passed when the editor creates a sequence".

Nothing takes it today. `project.new` and `sequence.create` make a video track `V1` whose `magnetic` flag is off, and neither has a parameter for it. The only way to get a magnetic main track is `track.set_magnetic` afterwards, which is what the track's context menu does (flow F37).

So the preference cannot be applied without one of two things, and both are a decision for the product owner:

- A new parameter on `project.new` and `sequence.create`. That is a change to two command schemas, which needs an ADR.
- The editor calling `track.set_magnetic` after it creates a sequence. That needs no schema change, but with the preference's default (on) every project made in the app would start with a magnetic main track. Today none does, and the timeline flows (F04, F05, F38, F39, F61 and others) were written against a free main track.

WP-2A.13 ships the rest of Settings > Timeline without this row: no control that does nothing may land. The preference key `timeline.magnetic_main` and its row are added when this ADR is accepted, by the work package that applies them.

## Decision drivers

- Agents, the CLI and the editor must be able to make the same sequence with one call (parity, ADR-0002).
- Creating a project is one undo step; a second command would add a step a person can undo by accident.
- A change of default for every new project must be a deliberate product decision, with the flows updated in the same change.
- The project format must not change: `Track.magnetic` already exists.

## Considered options

1. **A `main_magnetic` parameter on `project.new` and `sequence.create` (leading):** optional boolean; absent keeps today's behaviour (off). The editor passes the preference. One call, one undo step, the same for agents.
2. **The editor calls `track.set_magnetic` after creating a sequence, in one batch transaction:** no schema change, one undo step. Agents and the CLI need two calls for the same result, and the editor's behaviour is not visible in the command's own schema.
3. **Change the engine's default to magnetic:** no parameter. Every caller's new sequences change, including agents' and tests', with no way to ask for the old behaviour in the same call.
4. **Drop the preference:** the main track stays free unless the person turns the magnet on per track. The catalog row and the WBS line would be removed.

Option 3 changes behaviour for every client at once and is rejected. Option 2 is workable but hides a product default in the editor. Option 4 is the owner's to choose.

## Decision

Accepted by the product owner on 2026-11-27: the preference stays (new sequences made in the app start with a magnetic main track, default on), through option 1, an optional `main_magnetic` parameter on `project.new` and `sequence.create`; absent keeps today's behaviour (off). Built under issue #240.

**Decided by:** the product owner, on two questions:

1. Should new sequences made in the app start with a magnetic main track by default (the preference's catalog default is on)?
2. If the preference stays: option 1 (a `main_magnetic` parameter) or option 2 (the editor follows up with `track.set_magnetic`)?

**Pass criteria for the follow-up work package** (either option): a unit test that a sequence created with the value on has a magnetic `V1` and with it off has none, as one history entry; the parity test covers the new parameter (option 1); flow F25 gains "turn the preference off, create a project: the main track is not magnetic"; every timeline flow that assumes a free main track on a new project either turns the preference off first or is rewritten for the magnetic track.

**Until #240 lands:** there is no preference key and no row in Settings > Timeline; the catalog lists the row as waiting on this ADR. `project.new` and `sequence.create` are unchanged.

## Consequences

**Positive**

- No command schema and no default behaviour changes without a decision.
- The rest of the Timeline page ships now.

**Negative / costs**

- Settings > Timeline has no "Main track is magnetic in new sequences" row until this is accepted. The settings catalog says so.
- A follow-up work package is needed for the parameter (or the follow-up call) and for the flows.

**Follow-ups**

- A work package for the chosen option, with the pass criteria above. It adds the preference key `timeline.magnetic_main`, its row, strings and tests.
- If option 1: `cargo xtask gen`, the command catalog and the MCP token budget (two parameter descriptions).

## Links

- Architecture: `../01-architecture.md` §5.1, §16
- Work package: `../wbs/P2-core-editor.md` (WP-2A.13)
- Settings catalog: `../reference/settings.md` (`timeline.magnetic_main`)
- Related: [ADR-0023](0023-timeline-edit-semantics.md), flow F37 in `../user-flows.md`
