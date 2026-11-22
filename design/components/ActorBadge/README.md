ActorBadge shows who made an edit: the user, an AI agent, or a plugin; the agent variant uses agent blue with a sparkles icon.

**When to use**
- History and undo lists, version diffs, comment-like activity feeds.
- Hover cards that explain a clip's origin ("Agent: claude-code").
- Not for generated media provenance on tiles (use the AI badge) or status (use StatusChip).

**Anatomy**
`.cw-actor` (pill, `radius-full`, type: caption, `space-1` icon gap) > `.cw-icon.cw-icon--sm` + text. Variants: default "You" (`surface-3`, user icon), `--agent` (`agent-subtle` fill, `agent` border, `agent-text`, sparkles, "Agent: claude-code"), `--plugin` (transparent, `text-secondary`, plug icon, "Plugin: acme.lut-pack"). In a list row it sits left of the action text and the timecode (mono, `text-muted`).

**The consumer provides**
Actor kind (`user`, `agent`, `plugin`), the identifier shown after the colon (agent client or plugin id), and the action text and time in the row.

**States**
Static label. Rows hover to `surface-3` and can be selected with `marigold-subtle`. The badge itself never changes between states.

**Keyboard & accessibility**
Text is always present ("You", "Agent: claude-code"), so the icon is decorative. In lists use `role="list"`/`role="listitem"`. Actor identity is announced before the action.

**Command mapping**
Each history entry records the actor on the engine transaction (`actor: user | agent:<client> | plugin:<id>`), the same for UI and MCP commands, so the badge is derived data, not a UI choice. Undo/redo apply regardless of actor.

**Do**
- Use `agent` color only for agent actors; plugins stay neutral.
- Keep the identifier in the text ("Agent: claude-code").
- Align badges to the same left edge in lists.

**Don't**
- Don't use marigold for "You"; marigold is not a person color.
- Don't abbreviate the agent name beyond its client id.
- Don't render the badge without its words.
