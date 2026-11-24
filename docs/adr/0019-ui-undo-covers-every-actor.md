# ADR-0019: The editor's Undo and Redo cover every actor's changes

- **Status:** Accepted
- **Date:** 2026-11-24
- **Deciders:** product owner (decision), integrator (recorded)
- **Supersedes:** none (refines the undo rule of WP-1B.04)
- **Architecture refs:** `docs/01-architecture.md` §5.2, §14b, §16; `docs/user-flows.md` F7, F15

## Context

Every transaction is tagged with the actor that made it (`user`, `agent:<label>`, `plugin:<id>`, `system`), and the engine keeps undo and redo stacks per actor (§5.2). `edit.undo{actor}` and `edit.redo{actor}` already accept `*`, meaning "anyone's most recent change".

The editor sent `edit.undo` without an actor, so Ctrl+Z, Edit > Undo and the top bar's Undo button only reverted the person's own changes. The user-flow suite (F15, WP-1O.05) showed what that does in practice. An agent attached to the running engine adds and trims a clip. The person presses Ctrl+Z, and the editor tries to undo the person's earlier import instead. The engine refuses it because the agent's clip now uses that media, and the person gets a raw `history_target_modified` message. Meanwhile the button reads "Undo Import media".

Cutweave is built for people and agents editing the same timeline at the same time, so this question will come up constantly.

## Decision drivers

- A person watching an agent edit expects Ctrl+Z to take back the last change they saw happen, whoever made it.
- The Undo button must say what it will undo before it is pressed.
- Agents, MCP clients and the CLI must still be able to undo only their own work.

## Considered options

1. **The editor's Undo and Redo act on anyone's latest change** (chosen by the product owner).
2. **Keep the editor on the person's own history and add a separate "Undo agent change" action.** Rejected: two undo actions to choose between, and the common case (take back what just happened) needs the less obvious one.
3. **Keep it as it is.** Rejected: the F15 behaviour above.

## Decision

- **The editor uses `*`.** Ctrl+Z, Ctrl+Y / Ctrl+Shift+Z, Edit > Undo and Redo, and the top bar's buttons send `edit.undo{actor:"*"}` and `edit.redo{actor:"*"}`. That is the most recent change by anyone, or the most recently undone change for redo, under the existing undo rule (WP-1B.04): a change whose targets were modified since is refused with `Conflict{history_target_modified}`.
- **Engine-side history is unchanged.** Stacks stay per actor. Agents, MCP and the CLI keep `edit.undo{actor}` for their own changes, and the default for a connection stays its own actor.
- **The engine reports the shared view.** `edit.history` and the `edit.history_changed` event carry, next to the per-actor stacks, what `*` would undo and redo next: depth, label and the actor that made it. This is an additive change to their output schemas. The editor drives its Undo/Redo state and labels from it.
- **Labels name the step, and for someone else's change, who made it.** For example "Undo Move clip" for the person's own change, and "Undo Insert clip (Claude)" for an agent's. A refused undo is explained in plain words, not as a raw conflict code.

## Consequences

- **Positive:** Ctrl+Z does what a person expects in a shared session, and F15's "Ctrl+Z in the UI undoes the agent's change" flow can pass. Agents keep a private undo.
- **Negative:** a person can undo an agent's edit in the middle of the agent's turn. The agent then sees its change reverted; it learns that from `doc.patched` and `edit.history_changed` like any other concurrent change.
- **Follow-ups:**
  - Implement the schema addition, the editor change and the F15 step.
  - The in-app copilot (§14b) can still offer "undo the agent's last change" by passing its actor.
