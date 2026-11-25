# ADR-0029: Opening or closing a project never drops unsaved work unasked, and says that it is opening

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (drafted by the issue #161 fix)
- **Supersedes:** none (amends `project.open` and `project.close`, WP-1I.04, and the session events of arch §7)
- **Architecture refs:** `docs/01-architecture.md` §5.1, §7, §14

## Context

The product owner launched the app on an untitled document, started editing, and an agent ran `cutweave exec … project.open` on another project (issue #161). The other project appeared later with no sign that anything was opening, and the edits were gone. `project.open` closed the current document with no check: an untitled document has no file and no journal, so its work was lost for good, and a project with autosave off kept its work only in the journal, out of sight until that project was opened again. The command was even annotated `destructive = false`. `project.close` with `save` absent did the same. Only the app's own File > Open asked Save / Don't save first.

Nothing told the app that an open had started either: the engine sent `doc.reset` and `project.opened` when it was done, so the window kept accepting edits for a document that was about to be replaced.

## Decision drivers

- An agent, the CLI, MCP or a plugin must never lose the person's work without the person's say.
- The agent must learn why it was refused, in words and in a machine code, and what to do next.
- The person must see that a project is opening, and who opens it, and must not edit a document that is about to be replaced.
- One rule for every caller (parity, ADR-0002): the app goes through the same command.

## Considered options

1. **Refuse unless the caller opts in** (chosen): `project.open` fails with `Conflict{reason: unsaved_changes}` when the close would leave changes unsaved, unless it passes `discard_unsaved: true`; `project.close` does the same unless `save: false`. Two events frame an open.
2. **Ask in the app from the engine** (the engine holds the agent's call while the app asks Save / Don't save / Cancel). Rejected for now: a headless engine has nobody to ask, a call held open for minutes breaks agents' timeouts, and the engine would need a UI round-trip it has nowhere else. The refusal is the building block; an app-side "an agent wants to open X" prompt can come later on top of it.
3. **Save automatically before an agent's open.** Rejected: an untitled document has nowhere to be saved, and saving a project the person was not ready to save is a decision that is theirs.
4. **Refuse agents only, let the UI credential through.** Rejected: two behaviours for one command breaks parity, and the app's own Open must not lose work either.

## Decision

We will make `project.open` and `project.close` refuse, with `Conflict{reason: "unsaved_changes"}` and a message naming the project, any close that would leave changes unsaved without the caller saying so. Changes are at risk when the document is untitled and has committed changes (it has no file), or when a project has changes the close would not save (`save` absent and autosave off). Autosave on saves them first, as before. A read-only project, or one awaiting a recovery decision, has nothing unsaved to lose.

We will add `discard_unsaved: Option<bool>` to `project.open`. `true` closes the current project without saving, exactly as `project.close` with `save: false` (a project's work stays in its journal and is offered on its next open; an untitled document's is gone). `project.close` already has that say-so in `save: false`, so it gets no new parameter; its default now refuses instead of dropping. Both commands are annotated `destructive = true`.

We will add two session events. `project.opening{path, actor}` is sent when an open starts; `project.opened` (unchanged) or `project.open_failed{path, actor, code, reason, message}` ends it. `project.open_failed` is also sent when an open is refused for unsaved changes, so the app can tell the person even when an agent asked.

The app uses them: File > Open passes `discard_unsaved: true` only after the person chose Don't save; when the engine still refuses (it knows of changes the app did not count), the app asks and retries. While any open runs, the window shows "Opening <name>…" and takes no edits; an open or a refusal that came from an agent is said in a toast in agent colour.

**Decided by:** the product owner's review of the issue #161 fix. Pass criteria: the engine tests (refused open and close with the document unchanged, `discard_unsaved` and `save: false` drop as asked, autosave-on saves first, the events in order), the editor tests, and user flow F44 (an agent's open is refused while the person has unsaved work, the work stays and a toast says why; after a save the agent's open shows the project opening, then the project). Fallback: keep the refusal and the opt-in, drop the two events and the app's opening state.

## Consequences

**Positive**

- Unsaved work is never lost to an agent, the CLI, MCP or a plugin; the agent gets a machine reason and a hint to ask the person.
- The app shows every open, whoever started it, and the person cannot edit a document that is about to be replaced.
- `project.close` no longer drops an untitled document's work silently, also with `save: true` (which cannot save it).

**Negative / costs**

- A contract change: one parameter, one conflict reason, two events, two annotations; generated schemas and types change.
- Callers that relied on `project.close` or `project.open` dropping changes by default must now pass `save: false` or `discard_unsaved: true`.
- An agent that wants to open a project while the person has unsaved work must ask them; nothing in the app offers to settle it for the agent yet.

**Follow-ups**

- An in-app prompt for an agent's refused open ("Claude wants to open Film: Save / Don't save"), building on `project.open_failed`.
- The same say-so rule for any later command that replaces the document.

## Links

- Architecture: `../01-architecture.md` §5.1, §7, §14
- Issue #161; WP-1I.04 (session commands), WP-1M.07 (the app's unsaved-changes dialog)
- ADR-0002 (one command bus), ADR-0016 (credential classes), ADR-0018 (user flows: F44)
