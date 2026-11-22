Button triggers a single editor command; it comes in primary (marigold), secondary, ghost and danger variants at three heights.

**When to use**
- One primary button per surface for the action that will happen next (Export, Generate, Accept).
- Secondary for everything else; ghost for Cancel and low-emphasis toolbar-adjacent actions.
- Danger only for destructive, hard-to-undo actions.

**Anatomy**
`.cw-btn` > optional `.cw-icon` (icon-md) + label (type: body-strong; `--sm` uses label). Variants: `--primary`, `--ghost`, `--danger`, `--agent` (agent fill, for Accept only). Sizes: `--sm` = `control-sm`, default `control-md`, `--lg` = `control-lg`.

**The consumer provides**
Label text, optional leading icon, `type="button"`, the command id it dispatches, and `disabled` when the command is unavailable.

**States**
Default, hover (`surface-3`; primary/danger brighten), focus-visible (`focus-ring`), disabled (`text-disabled`, `border-subtle`). Pressed is not persistent; use IconButton or SegmentedControl for toggles.

**Keyboard & accessibility**
Native `<button>`; Enter/Space activate. Minimum width and height meet `hit-min`. Icon-only buttons must be IconButton with an `aria-label`. Disabled buttons keep their label visible and are skipped by Tab.

**Command mapping**
Every button dispatches a registered engine command (for example Split -> `clip.split`, Export -> `export.start`); the same command is reachable by MCP. Never wire a button to UI-only state.

**Do**
- Use `--lg` only in dialogs and AI Studio, where `control-lg` is the standard.
- Pair marigold fills with `on-marigold` text, never `text-primary`.
- Keep label verbs short and specific ("Add to timeline").

**Don't**
- Don't place two primary buttons side by side.
- Don't use `--agent` for anything but accepting agent work; agent color never marks primary actions.
- Don't hand-tune hover colors; use `surface-3`.
