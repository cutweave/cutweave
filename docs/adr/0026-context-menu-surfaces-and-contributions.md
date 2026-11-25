# ADR-0026: Context menus are built from commands, per named surface, and plugins contribute to the same surfaces

- **Status:** Proposed
- **Date:** 2026-11-25
- **Deciders:** Cutweave maintainers (drafted from the context-menu research for the integrator)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §2 (desktop shell), §5.1, §15, §16

> Process: [docs/06-dev-process.md](../06-dev-process.md) §9. Only maintainers accept this ADR.

## Context

The editor has no context menus (research: [09-context-menus.md](../research/09-context-menus.md)). The product owner asked for almost everything on screen to react to a right-click. Menus will be written by many WPs over P2 to P6 and by plugins (WP-6B.03: manifest `contributes.menus`). Three things are hard to change once plugins exist: the names of the surfaces a menu attaches to, the context keys a `when` clause may read, and the rule that menu items are commands.

## Decision drivers

- Every mutation is a registered command (ADR-0002, ADR-0005), so menus, palette and keymap must come from one source and agents get parity for free.
- The user-flow suite drives the real UI (ADR-0018); native context menus cannot be driven.
- Plugins must be able to add items without editor code changes and without reaching beyond their permissions.

## Considered options

1. **In-app menus, items declared as data referencing command ids, surfaces named by stable IDs (chosen).**
2. **Native Electron context menus.** Rejected: Playwright cannot click them, they cannot show agent blue, disabled reasons or live keymap hints, and the timeline and Preview are canvases whose targets only the renderer knows.
3. **Hand-written menus per component (closures).** Rejected: they drift from the palette and keymap, bypass the command registry's parity tests and become several undo steps when a closure calls several commands.

## Decision

**Decided by:** WP-2H.12 (framework), consumed by WP-6B.03. It becomes Accepted when WP-2H.12 passes: (a) a fixture command carrying menu hints appears in its surface with no UI code change; (b) the surface-table walk test finds every `command` in the catalog or the UI command table; (c) the Windows user flow F42 drives menus with real clicks and the Menu key; (d) axe reports 0 violations on each opened menu. If (c) fails because Radix cannot open at an arbitrary point on the canvas, the fallback is a synthetic `contextmenu` event dispatched on the surface element; the decision below does not change.

We will:

- Draw every context menu in the renderer with the design system's `Menu`, on all platforms. macOS keeps its native application menu bar only.
- Declare menus as data: for each **surface ID**, ordered groups of `{command, args(ctx), when, order}`. Label, icon, destructive flag and `undoable` come from the command catalog; shortcut hints from the keymap. Editor-side UI and host actions (layout, clipboard text, reveal) live in a UI command table with the same shape and never change the document.
- Keep a first set of **surface IDs** as a public contract: `dock.tab`, `dock.empty`, `bin.card`, `bin.folder`, `bin.background`, `bin.chip`, `preview.canvas`, `preview.item`, `source.view`, `timeline.clip`, `timeline.edge`, `timeline.keyframe`, `timeline.lane`, `timeline.track`, `timeline.ruler`, `timeline.playhead`, `timeline.marker`, `inspector.section`, `inspector.row`, `chrome.save`, `chrome.export`, `chrome.toast`, `field.text`. New IDs may be added without an ADR; renaming or removing one needs one.
- Keep a first set of **`when` context keys** as a public contract: `selection.count`, `selection.kind` (`video`, `audio`, `image`, `text`, `compound`, `mixed`), `item.linked`, `item.hasAudio`, `item.enabled`, `track.locked`, `playhead.inItem`, `clipboard.hasItems`, `media.used`, `media.offline`, `engine.live`, `project.readOnly`. The evaluator is a small boolean expression language (`&&`, `||`, `!`, `==`, `!=`); an unknown key is false and logged.
- Let plugins and first-party feature WPs contribute through one registry: `register({surface, command, group, when, order})`. Plugin items are rendered in a trailing group, at most three per plugin before an overflow submenu, with plain-text labels and only for commands the plugin may run.
- Resolve the target once when the menu opens, by IDs and never indexes (ADR-0004). A right-click on an unselected item selects it first (`view.select`, not undoable).
- Allow host actions (reveal in the file manager, copy text or an image, dev-only Inspect element) only through allowlisted, validated IPC, and list them in an exemption file with a reason, like `schema/parity-exempt.toml`.

## Consequences

**Positive**

- A command added to the registry can appear in a menu, the palette and the keymap with one declaration, and agents can run the same command by name.
- Plugin menu items need no editor changes and cannot exceed the plugin's permissions.
- Menus are testable end to end with real input.

**Negative / costs**

- The surface IDs and context keys are now a compatibility surface; renames need an ADR and a manifest `host_api` bump.
- No OS spellcheck suggestions in text-field menus in the first version.
- The renderer owns more UI code (host, evaluator, tables) and each owning WP carries a table to maintain.

**Follow-ups**

- WP-2H.12, WP-2H.13 and the menu bullets in the WPs listed in the research document.
- WP-6B.03 adds `contributes.menus` to the manifest schema and the plugin docs (WP-6C.08).
- Update arch §23 (ADR list) and the ADR index when this changes status.

## Links

- Architecture: [§2](../01-architecture.md), [§15](../01-architecture.md), [§16](../01-architecture.md)
- Research: [09-context-menus.md](../research/09-context-menus.md)
- Design: [design/components/Menu/README.md](../../design/components/Menu/README.md)
- Related: ADR-0002, ADR-0004, ADR-0005, ADR-0010, ADR-0016, ADR-0018
