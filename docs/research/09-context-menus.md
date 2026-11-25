# 09 - Context menus across the editor (inventory, per-surface menus, framework, plan)

Date: 2026-11-25. Scope: what a person can right-click in Cutweave today, what each surface should offer, and how to build it once.
Marks: [E] = a registered command exists today; [WP] = planned by that work package; [M] = missing everywhere (needs a new command or a UI action); [H] = host or UI action with no document effect (no MCP parity needed; see section 5.3); [U] = unverified (from secondary sources or memory); [D] = decision or recommendation.
Trigger: product owner, 2026-11-25: "feels like we are missing it. almost everything on screen should react to context menu with appropriate actions, find out which needs what".
Plan: [section 7](#7-plan). Work packages: WP-2H.12, WP-2H.13 and bullets in 39 existing WPs ([wbs/P2-core-editor.md](../wbs/P2-core-editor.md)). Decision record: [ADR-0025](../adr/0025-context-menu-surfaces-and-contributions.md) (Proposed).

## 1. Findings in one paragraph

Nothing in the editor reacts to a right-click. The design system already ships the component (`Menu` with `mode="context"`, `packages/ui/src/components/Menu.tsx`, spec in [design/components/Menu/README.md](../../design/components/Menu/README.md)), but only its dropdown mode is used (Filters, Sort and Open recent). The one context-menu hook in the editor, the dock tab's `getTabContextMenuItems`, returns an empty list (`packages/editor/src/dock/Dock.tsx:104`). The timeline is a Canvas2D surface that ignores every button but the left and the middle ones (`packages/timeline/src/interaction.ts:406-415`), so right-click on a clip does nothing, not even select it. Electron shows no default menu either, so a text field has no Cut, Copy and Paste on right-click. The WBS mentions context menus in seven places (2A.10, 2A.11, 2B.05, 2F.03, 2F.08, 5F.05, 6D.03) and no WP builds the shared machinery or owns the surfaces that have no WP of their own (bin cards today, preview, tracks, ruler, chrome, text fields). The plan adds one framework WP and one rollout WP, and puts a menu bullet in each owning WP.

## 2. Inventory of today's surfaces

"Menu today" is what a right-click does now. "Dropdown" means a click-opened `Menu` exists, which is not a context menu. Sources: `packages/editor/src`, `packages/ui/src`, `packages/timeline/src`.

### 2.1 Dock and shell

| Surface | Where | Menu today | Notes |
|---|---|---|---|
| Panel tab (the group header with the panel title) | `dock/Dock.tsx` (`PanelTab`) | none | `getTabContextMenuItems={() => []}` is an empty stub. No close button yet (WP-2H.01). |
| Tab strip / group header empty area | dockview | none | `singleTabMode="fullwidth"`: the tab fills the header, so the strip is the tab. |
| Empty dock (all panels closed) | none yet | n/a | The empty state arrives with WP-2H.01 (F33). |
| Floating panel | `disableFloatingGroups` | n/a | Floating is off in P1; WP-2H.01 turns it on. |
| Sash (splitter) | dockview | none | Double-click to reset size is a convention worth adding (2H.13). |
| Title bar (drag region) | `shell/TopBar.tsx` | OS system menu on Windows [U] | Leave to the OS (Restore, Move, Size, Close). Do not override. |
| Menu bar (File, Edit, View, Help) | `shell/AppMenuBar.tsx`, `ui/MenuBar` | click menus, not context | Not in scope. |
| Undo and Redo buttons | `TopBar.tsx` | none | Tooltip names the step. A menu of recent steps is WP-2H.11's History panel. |
| Workspace switcher, Reset layout button | `TopBar.tsx` | none | |
| Save status (dirty dot, chip) | `shell/SaveStatus.tsx` | none | |
| Export button and chip | `TopBar.tsx`, `export/ExportChip.tsx` | none | |
| Engine status chip | `shell/EngineNotices.tsx` | none | |
| Toast | `ui/Toast.tsx`, `shell/Toasts.tsx` | none | |
| Any text field (name, timecode, search, settings) | `ui/TextField`, `SearchField` | none | Electron has no default text menu. Cut, Copy, Paste and Select all exist only as keys and as `edit:run` IPC (arch section 2, desktop shell paragraph). |

### 2.2 Assets panel

| Surface | Where | Menu today | Notes |
|---|---|---|---|
| Category strip (Media, Audio, Text, Stickers, Effects, Transitions, Filters, Templates, Captions, AI Studio) | `panels/AssetBrowserPanel.tsx` | none | Only Media has content in P1. |
| Bin card: video | `panels/MediaPanel.tsx` `CardView` | none | Double-click inserts; Space plays; Delete removes (confirm dialog); hover-scrub. |
| Bin card: audio | same | none | Same actions; no picture. |
| Bin card: image | same | none | Still-duration rules arrive with WP-2F.05. |
| Card with a problem (offline, failed analysis) | same (`cw-media__status`) | none | Relink arrives with WP-2F.03. |
| Several cards selected | none | n/a | Multi-select arrives with WP-2F.08. The bin is single-selection today. |
| Bin background (grid gap, below the cards) | `cw-media__scroll` | none | |
| Empty bin | `cw-media__empty-actions` | none | Buttons: import. |
| Search box | `ui/SearchField` | none | Text field menu (2.1). |
| Filter chip | `ui/FilterChip` | none | Click removes. |
| Filters and Sort buttons | `MediaPanel.tsx` | **dropdown** | Radix `Menu mode="dropdown"`. |
| Grid / List toggle, Import, Remove buttons | `MediaPanel.tsx` | none | |

### 2.3 Preview

| Surface | Where | Menu today | Notes |
|---|---|---|---|
| Preview canvas (sequence) | `preview/PreviewCanvas.tsx` | none | WebGPU or JPEG fallback canvas inside the stage. |
| Overlay (safe areas, selection outline, readout) | `preview/PreviewOverlay.tsx` | none | `aria-hidden`; transform handles arrive with WP-2H.06. |
| Source view (picture, its header and timecode) | `panels/SourceHeader.tsx` | none | Header has Insert and Back to timeline buttons. |
| Preview toolbar (aspect switcher, safe-area toggle) | `panels/PreviewPanel.tsx` | none | |
| Transport bar (time, play, step, volume, quality, fullscreen) | `panels/TransportBar.tsx` | none | Timecode readout, quality badge and volume slider each deserve a small menu. |

### 2.4 Timeline

The timeline body is one `<canvas>`; `packages/timeline/src/hit-test.ts` already names the zones a menu needs.

| Surface | Hit zone | Menu today | Notes |
|---|---|---|---|
| Clip: video | `body` | none | Right-click does not even select (`interaction.ts` drops buttons other than 0 and 1). |
| Clip: audio | `body` | none | Waveform clip. |
| Clip: image, text, compound, linked, multi-selected | `body` | none | Linked groups arrive with 2A.05, text with P3, compound with 2B.04. |
| Clip edge | `left-edge`, `right-edge` | none | Trim handles. A menu here is useful for "Ripple trim to playhead" style items. |
| Keyframe diamond | `keyframe` | none | The hit zone exists; lanes arrive with 2C.05. |
| Empty lane (gap or empty track area) | `lane` | none | Carries `trackId` and `time`. |
| Track header | `header` | none | Toggles (`header-toggle`: lock, mute, hide) are click targets. |
| Ruler | `ruler` | none | Scrub target. |
| Playhead flag | `playhead` | none | |
| Below the last track | `none` | none | |
| Timeline toolbar | `panels/TimelinePanel.tsx` | none | Select, razor and ripple tools are disabled in P1. |
| Markers, in/out | not drawn yet | n/a | WP-2A.02 (model), WP-2A.10 (UI). |
| Sample lanes (no engine) | `SampleLanes` | none | |

### 2.5 Inspector and settings

| Surface | Where | Menu today | Notes |
|---|---|---|---|
| Section header | `ui/InspectorSection` | none | Collapsible; an "undo" reset button appears when modified. |
| Number field (scrubber) | `ui/NumberScrubber` | none | Double-click edits. |
| Text, time and name fields | `panels/InspectorPanel.tsx` | none | Text field menu (2.1). |
| Property label / row | `ui/InspectorRow` | none | Row reset button (spec only). |
| Keyframe toggle | `ui/KeyframeToggle` | none | |
| Media picture in the Inspector (scrub bar) | `inspector/MediaPreview.tsx` | none | |
| Media info rows (path, codec, size) | `panels/InspectorPanel.tsx` `MediaInfo` | none | A path is worth copying. |
| Settings row | `ui/SettingRow`, `settings/*` | none | Reset exists as a button. |
| Settings page tab | `ui/SettingsLayout` | none | |
| Export dialog fields | `export/ExportDialog.tsx` | none | Text field menu only. |
| Agents and MCP rows (folders, clients, setup text) | `settings/agents.tsx` | none | Copy is a button. |

## 3. Planned surfaces that will need a menu

Each is named with the WP that owns it. The WP gets a short menu bullet (section 7.3).

| Surface | WP | Notes |
|---|---|---|
| Clip, lane, ruler, playhead, markers, in/out on the timeline | 2A.10 (and 2A.02, 2A.05, 2A.06, 2A.11, 2A.14) | Biggest surface. |
| Sequence tab | 2B.03 | Open, close, duplicate, rename, settings. |
| Compound clip, breadcrumb | 2B.05 | |
| Keyframe diamond and lane, curve editor, graph editor | 2C.05, 2C.07 | Interpolation submenu, delete, copy. |
| Speed handle / clip speed | 2D.01 | Items live in the clip menu. |
| Fade handle, volume line, mixer fader | 2E.05 | Reset, set to 0 dB, mute. |
| Bin tile with health badge, folder tile, breadcrumb, sidebar | 2F.03, 2F.06, 2F.08 | |
| Export queue row | 2G.04 | Cancel, retry, reveal, copy error. |
| Command palette row | 2H.04 | Bind shortcut, copy command id. |
| Shortcut editor row | 2H.05 | Reset binding, remove, copy. |
| Preview transform handles, crop box | 2H.06 | Reset transform, fit, fill. |
| History row | 2H.11 | Go to this step, copy label. |
| Panel registry (View > Panels) | 2H.01 | |
| Text clip on the canvas, text presets | 3A.07, 3C.02, 3C.05 | Edit text, copy style, apply preset. |
| Transcript word, selection, sentence | 3D.06, 3F.03 | Delete words, cut from timeline, copy. |
| Caption row and style card | 3E.11 | Split, merge, restyle. |
| Cleanup review list (silences, fillers) | 3F.05 | Accept, skip. |
| Library cards (audio, stickers, emoji) | 3G.05 | Preview, add, favourite. |
| Effects, Transitions, Filters browser cards; clip effect stack row | 4A.09 | Favourite, apply, apply to all, copy. |
| Animations tab cards | 4E.05 | |
| Color panel (wheel, curve point) and scopes | 4F.06 | Reset, copy grade, paste grade. |
| Mask shape and handle overlay | 4G.02 | Invert, duplicate, delete. |
| Speed curve point | 4H.04 | |
| Tracker result and keyframes | 4I.05 | |
| AI Studio: job row, result gallery tile, provenance drawer | 5F.04, 5F.05, 5F.03 | Send to timeline, extend, fill gap (5F.05 already says "timeline context menu"). |
| Shorts shortlist candidate | 5H.04 | |
| Chat message, tool chip, proposal | 6I.07, 6I.08 | Copy, retry, undo turn. |
| Plugin panels and plugin-contributed items | 6B.03 | Manifest `contributes.menus`. |
| Plugin browser row | 6C.05 | |
| Template card, slot marker | 6D.03, 6D.04 | "Make replaceable" is already planned as a right-click in 6D.03. |

## 4. Conventions in other editors

Web research was thin: the vendors publish no full list of their context menus, so the lists below combine the sources that were found with long-standing conventions [U]. Treat the detail as a starting point for the WP-2H.07 UX review, not as a spec.

- **Premiere Pro.** The clip menu is long and users complain about it; Adobe staff moved Media File Properties out of it because it was rarely used and said they were working to shorten it ([Adobe Community: context menu too long](https://community.adobe.com/feature-requests-730/right-click-context-menu-too-long-1327837); [Properties option missing](https://community.adobe.com/t5/premiere-pro-discussions/properties-option-missing/m-p/14996131)). The effects badge has its own short menu ([Adobe Community](https://community.adobe.com/t5/premiere-pro-discussions/why-is-the-context-menu-changing-every-time-i-right-click-on-a-clip-in-the-timeline/td-p/13539095)). Lesson: keep the clip menu short and give small targets (badge, edge, keyframe) their own menus.
- **DaVinci Resolve.** Menus are per surface: the timeline clip, the media pool (a clip, a bin, the background) and the timelines list each have their own, and newer AI actions are added as items on the selected clip ([Resolve 20 New Features Guide](https://documents.blackmagicdesign.com/SupportNotes/DaVinci_Resolve_20_New_Features_Guide.pdf)). The manuals describe the Cut page and menu structure ([Resolve manuals index](https://ltbits.github.io/davinci-resolve-manuals/DR18/DR18-RM04-TheCutPage.pdf)). Lesson: the media pool menu is the model for the bin, with "Create timeline using selected clips" as a headline action.
- **CapCut desktop.** Guides disagree on where actions live. Freeze is in the toolbar above the timeline, while Detach audio or Extract audio appears in the clip's right-click menu, depending on version ([TourBox: freeze](https://www.tourboxtech.com/en/news/how-to-freeze-a-frame-in-capcut-pc.html); [Hollyland: detach audio](https://www.hollyland.com/?p=328322); [Filmora: speed and freeze](https://filmora.wondershare.com/basic-video-editing/capcut-freeze-frame.html)). Lesson: CapCut users expect the clip menu to hold the one-click edits (split, delete, speed, freeze, extract audio, copy) next to the toolbar. The repo's own CapCut research ([01-capcut-features.md](01-capcut-features.md)) has no right-click list.
- **Final Cut Pro.** Apple documents keyboard shortcuts and the shortcut menu is not listed in the pages found ([Keyboard shortcuts in Final Cut Pro](https://support.apple.com/guide/final-cut-pro/ver90ba5929/mac)); its convention is Control-click on a clip, with Expand Audio and Show Audio Lanes as clip-menu items [U].
- **Kdenlive** (open source, documented in full): the clip menu has Copy, Duplicate, Paste Effects, Remove Effects, Group and Ungroup, Edit Duration, Disable clip, Delete, Extract, Cut, Replace with Bin Selection, Change speed and Markers; the empty-lane menu has Paste, Insert and Remove Space, marker items and Identify Gaps ([Kdenlive right-click menus](https://docs.kdenlive.org/en/cutting_and_assembling/right_click_menu.html)). Useful as a complete reference for what a clip and a lane menu can hold.

Conventions all of them share, which the plan adopts:

1. Right-click on an unselected item selects it first; on a selected item it keeps the selection and acts on all of it.
2. One menu per surface, and a small target gets its own short menu.
3. Order by frequency: the edit verbs first, clipboard next, then properties and "send to", then destructive items last, each group separated.
4. Shortcuts are shown at the right edge, from the live keymap.
5. Items that apply to the target but cannot run now are disabled (Paste with an empty clipboard, Split with the playhead outside the clip); items that make no sense for the target's kind are left out (audio fades on a text clip).
6. Delete is last and separated.

## 5. Cross-cutting design

### 5.1 One framework, three layers

| Layer | Where | Status |
|---|---|---|
| Visual component: `Menu` (`mode="context"`), `MenuBar`, design spec | `packages/ui` | **Exists.** Gaps: no submenu entry kind, no controlled open at a point (Radix `ContextMenu` opens only from a DOM `contextmenu` event), no disabled-reason text, no touch long-press. The design spec allows one submenu level. |
| Host: one `ContextMenuHost` mounted in `EditorShell`, a `useContextMenu(surface)` hook for DOM surfaces, and an imperative `openContextMenu({surface, point, target})` for the canvas | `packages/editor/src/menus/` | New (WP-2H.12). |
| Surface tables: for each surface, an ordered list of groups and items | `packages/editor/src/menus/surfaces/*.ts` | New; filled by WP-2H.13 and by each owning WP. |

[D] Keep Radix `ContextMenu`/`DropdownMenu` as the engine (it gives roles, typeahead, focus return and collision handling, and axe already passes on it). The host renders `Menu mode="dropdown"` in controlled mode against a zero-size anchor placed at the pointer, so one code path serves DOM surfaces, the canvas and the keyboard. A DOM surface that just wants a menu wraps itself in `<ContextSurface surface="bin.card" target={...}>`, which only forwards `contextmenu` events to the host.

### 5.2 Items come from commands (one source for menus, palette and keymap)

A surface table row is data, not a closure:

```
{ command: "clip.split", group: "edit", when: "item.kind == video && playhead.inItem", args: (ctx) => ({ item: ctx.item.id, at: ctx.playhead }) }
```

- **Label, icon, summary, undoable and the destructive flag** come from the generated catalog (`schema/commands.json`, `packages/protocol/src/generated/commands.ts`). The label is the registry's verb phrase; the surface table may override it only to add the object ("Split at playhead", "Delete 3 clips").
- **Shortcut hint** comes from the keymap engine (WP-2H.05) through `keysFor(commandId)`; before it lands, from today's tables (`shortcuts/bindings.ts`).
- **Enabled state** is computed from the same `when` evaluator the palette uses (WP-2H.04): a small typed context (selection count and kinds, item kind, linked, track locked, playhead inside the item, clipboard has items, media used, engine live, read-only). The evaluator and the context keys are specified in ADR-0025 because plugins will write `when` clauses against them.
- **UI-only and host actions** (close a panel, rename in place, reveal in Explorer, copy text) are entries in an editor-side `UiCommandTable` with the same shape (id, label key, icon, run). They appear in the palette and keymap too, so "one source" holds for them. They never mutate the document.
- **Everything that changes the document is a registered command applied through `Tx`**, so each menu choice is one undo step and an agent can do the same thing by name. A menu item that today would be a UI closure over several commands is a bug; add the composite command (section 6 lists the gaps).
- **A test walks every surface table**: each `command` must exist in the catalog (or in `UiCommandTable`), each document-changing one must be MCP-reachable (the 1O.02 parity rule), and each label key must exist in the `en` catalog (i18n lint). A fixture command with a `menu`/`when` hint must appear in a menu with no UI code change (the 2H.04 criterion, extended to menus).

### 5.3 Parity: what agents see

Menus add no new MCP surface: a menu item is a command with arguments filled from the context, so MCP, CLI and RPC parity are automatic for every document-changing item. Two kinds of item are outside the rule, and the test keeps them on an explicit allowlist with a reason (the same pattern as `schema/parity-exempt.toml`):

- **Host actions** that touch the OS and not the project: Reveal in Explorer, Copy to clipboard (text), Open in the default app, Inspect element (dev builds). They go through allowlisted, zod-validated IPC like `app:about` and `edit:run` ([arch section 2, desktop shell paragraph](../01-architecture.md)); main resolves paths itself and never takes a path from the renderer that the project does not already hold.
- **View-state items** (selection, zoom, playhead, panel layout) use `view.*` where it exists (`view.select`, `view.set_playhead`, `view.set_zoom`), and `workspace.*` for layout, which is user config and not agent-controlled.

An agent that wants "what can I do with this clip" uses `cutweave_search_commands` and `item.get`; it needs no menu endpoint. The "Ask agent..." entry (design variant `agent`) opens the chat (WP-6I.07) with the selection as context; until then it is not shown.

### 5.4 Selection and target rules

[D] These follow the conventions in section 4 and apply to every surface.

- A menu acts on a **target**: `{surface, ids[], point?: {time, trackId}, element}`.
- Right-click on an item that is not in the selection: the selection becomes that item (`view.select{mode:"replace"}`, not undoable), then the menu opens.
- Right-click on an item that is in the selection: the selection is kept and the menu acts on all of it. Labels count ("Delete 3 clips"). An item that cannot act on several (Rename, Properties) is disabled with a reason, not hidden.
- Right-click on an empty lane or the bin background: the selection is **not** cleared by the timeline (the menu acts on a place, `point`, not on the selection) and **is** cleared by the bin (Explorer convention).
- Linked clips: the menu acts on the whole group by default (the engine applies group rules, 2A.05); "Unlink" is offered when the target is linked.
- Locked tracks: items that edit are disabled with the reason "Track is locked"; the engine's `Locked` error stays the safety net.
- **Disabled reason:** a disabled item carries a one-line reason (`aria-describedby`, shown as a tooltip on hover or focus) so users learn why; this is the "disable, don't hide" rule of the design spec.
- **Time anchors:** "Split at playhead" uses the playhead (as the S key does). Items that use the click position are named for it ("Add marker here", "Paste here") and use the lane hit's `time`, snapped as the pointer would be.

### 5.5 Keyboard, accessibility, touch and pen

- **Keyboard open:** Shift+F10 and the Menu key. Chromium fires `contextmenu` on the focused element for DOM surfaces, which the hook handles. The timeline canvas, the dock tab and the Preview canvas are not natural DOM targets, so the host also listens for the keys on them and anchors the menu at the selection (clip rectangle's lower-left), the playhead, or the focused card.
- **Focus:** Radix traps focus while open and returns it to the anchor on close; the host restores focus to the surface element, not the zero-size anchor. Escape closes. Type-ahead and arrows come from Radix.
- **Roles and axe:** `role="menu"`, `menuitem`, `menuitemcheckbox`, `menuitemradio`; the menu's `aria-label` names the target ("Clip actions for interview_A.mov"). Each owning WP's axe check opens its menu. A menu on a canvas needs the canvas to have an accessible name and a keyboard path (WP-7C.02 builds the accessible timeline; the Shift+F10 route here is the first part of it).
- **Touch and pen:** touch long-press generates `contextmenu` in Chromium, so DOM surfaces get it free. On the canvas the pointer machine needs a long-press timer (500 ms, cancelled by a move over 8 px) that opens the menu and ends the gesture in `Cancelled`; a pen's barrel button reports `button: 2` and takes the right-click path. Both are secondary on Windows desktop and are tested with synthetic pointer events only.
- **Reduced motion and tokens:** from the existing `Menu` (no motion under reduced motion; `surface-2`, `shadow-popover`, `z-dropdown`).
- **Length rule:** 8 items or fewer per group is not needed; **at most about 12 visible rows** on a surface at once, with one submenu level for long lists (Speed, Interpolation, Move to, Add to collection). Premiere's long clip menu is the cautionary tale (section 4).

### 5.6 Native menus versus in-app menus on Windows

[D] **In-app menus on every platform, for context menus.** Reasons:

1. **ADR-0018** requires the user-flow suite to drive the app with real input; Playwright cannot click a native context menu, so native menus would force a test-only hook (forbidden).
2. The timeline and Preview are canvases; items need the hit-test result and the engine's state at open time, which is cheaper in the renderer than through IPC and back.
3. The menu bar is already in-app on Windows and Linux (arch section 2), and the design system defines the menu's look (agent variant, danger variant, shortcut hints from the live keymap, disabled reasons).
4. One implementation to test and to extend through plugins; a native menu cannot show agent blue or disabled reasons.

The costs are known: no OS-level spellcheck suggestions in text-field menus (the first version offers Cut, Copy, Paste, Select all, and spelling can be added from Electron's `context-menu` event parameters later) and slightly different look from the OS. macOS keeps its native application menu bar only; its context menus are the same in-app ones.

### 5.7 Plugin contribution points

Planned already: manifest `contributes.menus` (WP-6B.03, [P6](../wbs/P6-platform.md)). What the framework must provide now so that WP is a small addition:

- Surface IDs are stable strings (`timeline.clip`, `timeline.lane`, `bin.card`, `preview.canvas`, ...) listed in ADR-0025; the list is a **plugin-manifest contract** once published, so it changes only via ADR.
- A `MenuContributionRegistry` with `register({surface, command, group, when, order})`. First-party feature WPs (5F.05, 3F, 4A.09, 6I.07) use the same registry, so they are the first consumers and the plugin path is exercised before P6.
- Plugin items render in their own trailing group, below the built-in groups and above Delete when the surface allows it (the group's `order` is clamped), at most 3 per plugin before an overflow submenu "Plugin name".
- Labels are plain text (no markup) and are length-limited; plugin-supplied strings never reach `dangerouslySetInnerHTML`. The 6B.03 abuse tests already cover XSS in menu labels.
- `when` clauses are limited to the published context keys; an item whose command the plugin lacks permission for is not shown.
- Items are commands, so they appear in the palette and CLI with no extra work (arch section 15 table: "Contributions auto-surface everywhere").

### 5.8 Text fields and the "dead zone" rule

[D] Every `<input>`, `<textarea>` and contenteditable in the app gets one shared **text field menu** (Undo, Cut, Copy, Paste, Select all; Paste and Cut disabled when read-only) through the existing `edit:run` IPC, installed once by the host's capture handler so no field is missed. Elsewhere, a right-click outside any registered surface shows no menu (as today) and does not leak the browser's menu; **in development builds** the dead zone offers "Inspect element" so contributors keep a way in.

## 6. Recommended menus per surface

Status marks: [E] a registered command exists (name given); [WP-x] planned there; [M] missing everywhere (a new command or UI action; the plan says where it lands); [H] host action; [UI] editor-side UI action (`workspace.*` or similar, no document effect). Shortcuts are the NLE defaults of WP-2H.05 or what exists today; "(none)" means none planned. Order is by frequency; `---` is a separator.

### 6.1 Dock

**Panel tab** (`dock.tab`; also the single-tab group header)

| Item | Shortcut | Status |
|---|---|---|
| Close panel | (none) | [UI] `workspace.close_panel`, WP-2H.01 |
| Maximize panel / Restore | F11 is full screen, so (none) | [UI] WP-2H.01 (dockview maximize) |
| Float panel | | [UI] WP-2H.01 |
| Move to… (Left, Right, Bottom) | | [M][UI] new |
| --- | | |
| Reset layout | | [UI] WP-2H.01 (`workspace.reset`) |
| Save layout as… | | [UI] WP-2H.01 |

**Empty dock** (`dock.empty`): Reopen panels (a submenu of View > Panels), Reset layout [UI] WP-2H.01.
**Sash**: double-click resets the size, no menu [UI] 2H.13.

### 6.2 Assets

**Bin card** (`bin.card`, kind-aware; applies to the selected cards)

| Item | Shortcut | Status |
|---|---|---|
| Insert at playhead | Enter | [E] `timeline.insert` (as double-click does today) |
| Append to timeline | | [E] `timeline.append_clip` |
| Overwrite at playhead | `.` | [WP-2A.14] (`timeline.overwrite`, 2A.03) |
| Play in Source view | Space | [E] `view.set_source` + `view.play` (as the card's button) |
| --- | | |
| New timeline from selection | | [M] `sequence.create_from_media` (see 6.9) |
| --- | | |
| Rename | F2 | [WP-2F.07] (display names; folders WP-2F.08) |
| Move to… | | [WP-2F.08] `media.move` |
| Add tag… / Add to collection | | [WP-2F.06] |
| --- | | |
| Relink… (offline cards only) | | [WP-2F.03] `media.relink` |
| Regenerate proxies | | [WP-2F.03] |
| Reveal in Explorer / Finder | | [H] `shell:showItemInFolder` |
| Copy path | | [H] |
| Remove from project | Delete | [E] `media.remove` (confirm dialog stays) |

Kind rules: audio cards omit "Overwrite on video track"; image cards add "Duration…" [WP-2F.05]. The multi-selection menu reads "Insert 3 clips", "Remove 3 from project", and disables Rename.

**Bin background** (`bin.background`): Import… (Ctrl+I) [E] `media.import`; New folder (Ctrl+Shift+N) [WP-2F.08]; Paste (a media path list) [M] drop; Select all (Ctrl+A) [WP-2F.08]; Sort by and View as (submenus mirroring the dropdowns) [E] existing view state; Show hover scrub toggle [UI].
**Folder tile** (`bin.folder`): Open, Rename, Move to…, New folder inside, Delete… [WP-2F.08]. **Breadcrumb segment**: Open [WP-2F.08].
**Filter chip** (`bin.chip`): Remove filter, Clear all filters [UI] (WP-1M.12 state). **Search box**: text field menu.
**Category tab** (`assets.tab`): Close panel is not offered here; none planned. Right-click on a tab is a normal tab context (no menu) except "Show in separate panel" [M], deferred.
**Empty bin**: same as bin background.

### 6.3 Preview

**Preview canvas** (`preview.canvas`)

| Item | Shortcut | Status |
|---|---|---|
| Play / Pause | Space | [E] `view.play` / `view.pause` |
| Go to start / end | Home / End | [E] `view.seek` |
| --- | | |
| Safe areas, Grid, Center cross (toggles) | | [UI] `view.set{safe_areas}` exists as view state; grid and cross [WP-2H.06] |
| Zoom: Fit, 50%, 100%, 200% | | [E] `view.set_preview_size` (fit) / [WP-2H.06] |
| Quality: Auto, Full, Half, Quarter | | [E] `view.set_preview_size`/playback prefs [WP-2H.10] |
| --- | | |
| Copy frame to clipboard | | [M] `render.frame` + host clipboard (image) |
| Save frame as… (PNG) | | [M] `render.frame` + [H] save dialog |
| Add frame to media bin | | [M] `media.import_frame` (new) |

**Selected clip on canvas** (`preview.item`, WP-2H.06 handles): Reset transform, Fit to canvas, Fill canvas, Center, Flip horizontal/vertical, Bring to front/Send to back (track order) [WP-2C.03/2H.06; reset and flips need `item.set_props` combos; "Fit/Fill" are in WP-2C.08]. Not in P1.
**Source view** (`source.view`): Insert (`,`) [WP-2A.14], Overwrite (`.`) [WP-2A.14], Set in / Set out (I/O) and Clear marks [WP-2A.14], Match frame (F) [WP-2A.14], Back to timeline [E] `view.set_source{null}`, Copy timecode [H].
**Transport**: timecode readout: Copy timecode, Switch display (timecode / frames) [WP-2A.13 pref]; Volume: Mute, Reset to 100% [E] transport state; Quality badge: the quality list above.

### 6.4 Timeline

**Clip** (`timeline.clip`), the main menu. Target kind decides which groups show.

| Item | Shortcut | Status |
|---|---|---|
| Split at playhead | S (CapCut preset Ctrl+B) | [E] `clip.split` |
| Delete | Delete | [E] `item.delete` |
| Ripple delete | Shift+Delete | [WP-2A.03] `timeline.ripple_delete` |
| --- | | |
| Cut | Ctrl+X | [WP-2A.06] `timeline.copy` + delete |
| Copy | Ctrl+C | [WP-2A.06] `timeline.copy` |
| Paste attributes… | (unbound) | [WP-2A.06] `clip.paste_attributes` |
| Duplicate | Ctrl+D | [WP-2A.06] `item.duplicate` |
| --- | | |
| Speed ▸ (Normal, 2x, 0.5x, Custom…) / Reverse / Freeze frame | | [WP-2D.01] `clip.set_speed`, `clip.reverse`, `clip.freeze_frame` |
| Detach audio / Extract audio | | [WP-2E.03] `audio.extract` (video clips with sound) |
| Unlink / Link | (unbound) | [WP-2A.05] `item.unlink`, `item.link` |
| Group / Ungroup | | [WP-2A.05] (same family; confirm name) |
| Compound clip / Break apart | | [WP-2B.04] (menu in 2B.05) |
| Replace clip… | | [WP-2A.11] `clip.replace` |
| --- | | |
| Enable / Disable clip | | [E] `item.set_enabled` / `item.set_props{enabled}` |
| Rename… | F2 | [E] `item.rename` |
| Color label ▸ | | [M] `item.set_label_color` (the model has the field in arch 4.2; no command yet) |
| Reset properties… | | [M] `item.reset_props{items, group?}` (design spec names `clip.resetProperties`) |
| Normalize audio… / Fade in / Fade out | | [WP-2E.03], [WP-2E.05] |
| Add marker on clip | M | [WP-2A.02] `timeline.add_marker` (item marker) |
| --- | | |
| Reveal in media bin | | [M][UI] `view.select` the asset + scroll |
| Match frame (open in Source view) | F | [WP-2A.14] |
| Ask agent about this clip… | | [WP-6I.07] (agent variant, hidden until it lands) |
| --- | | |
| (Plugin group) | | [WP-6B.03] |

Group, Link, Speed, Detach, Replace, Compound and Reset show only when the WP that owns them has landed (the host hides unregistered commands; it disables registered ones that cannot run now). Audio-only clips: Detach and Reverse are left out; image and text clips: Speed and audio items are left out.

**Clip edge** (`timeline.edge`): Ripple trim to playhead, Trim to playhead (`[` and `]` in the CapCut preset), Extend to next clip, Roll to playhead [WP-2A.04 `clip.trim`, `clip.ripple_trim`, `clip.roll`; "to playhead" forms are `to` arguments; Extend to next edit is [M]].
**Empty lane** (`timeline.lane`, with `point`)

| Item | Shortcut | Status |
|---|---|---|
| Paste here | Ctrl+V | [WP-2A.06] `timeline.paste` |
| Add marker here | M | [WP-2A.02] |
| Delete gap / Close gap | | [WP-2A.03] `timeline.delete_gap` (shown on a gap between clips) |
| Select all clips on track / after here | | [E] `view.select` (build the id list) |
| --- | | |
| Add track above / below | | [E] `track.add` |
| Move playhead here | | [E] `view.set_playhead` |
| Zoom to fit | | [E] `view.set_zoom` (fit math in 2A.09/2A.10) |

**Track header** (`timeline.track`)

| Item | Shortcut | Status |
|---|---|---|
| Rename… | | [E] `track.set{name}` |
| Mute / Unmute; Solo; Hide / Show; Lock / Unlock (check items) | | [E] `track.set{muted,solo,hidden,locked}` |
| Make magnetic | | [WP-2A.02] `track.set_magnetic` |
| Track height ▸ (Small, Medium, Large) | | [WP-2A.02] `track.set_height` |
| --- | | |
| Add track above / below | | [E] `track.add` |
| Move up / Move down | | [WP-2A.02] `track.reorder` |
| Select all clips on track | | [E] `view.select` |
| --- | | |
| Delete track | | [E] `track.remove` (confirm when it has clips) |

**Ruler** (`timeline.ruler`): Add marker here (M), Set in point here (I), Set out point here (O), Clear in and out, Go to marker ▸, Time display: Timecode / Frames / Seconds, Zoom to fit, Zoom to selection [WP-2A.02 marker and in/out commands; display [WP-2A.13]; zoom [E] `view.set_zoom`].
**Playhead** (`timeline.playhead`): Split at playhead, Add marker at playhead, Set in/out here, Copy timecode [H], Go to start/end [E] `view.seek`.
**Marker** (`timeline.marker`): Edit… (label, color, duration), Go to marker, Delete marker, Convert to chapter [WP-2A.02 `timeline.move_marker`/`remove_marker`; edit is a form, label/color through `timeline.add_marker` update variant: [M] `timeline.update_marker`].
**In/out range bar**: Clear in and out, Export this range… [WP-2G.03], Lift, Extract [WP-2A.03].
**Keyframe diamond** (`timeline.keyframe`): Interpolation ▸ (Linear, Hold, Ease, Bezier…), Copy, Delete keyframe, Go to keyframe [WP-2C.03 `keyframe.*`, WP-2C.05].
**Below tracks / tool strip**: tool strip: none. **Sequence tab**: Rename, Duplicate, Settings…, Close, Close others [WP-2B.03].

### 6.5 Inspector and settings

- **Section header:** Reset section [M] `item.reset_props{group}`, Collapse all, Expand all [UI].
- **Property row** (label or field): Reset to default [M] `item.reset_props{props}`, Add keyframe / Remove keyframe [WP-2C.03 `keyframe.*`], Copy value, Paste value [H][M], Copy property as JSON (agents and bug reports) [H].
- **Number field:** text field menu plus Reset (as above).
- **Media info path:** Copy path [H], Reveal in Explorer [H].
- **Media picture (Inspector):** Play in Source view [E], Copy frame [M], Save frame as… [M] (same as the Preview).
- **Settings row:** Reset to default [E] preference store (WP-1M.11), Copy setting key [H]. **Settings tab:** none.
- **Text fields:** Undo, Cut, Copy, Paste, Select all [H].

### 6.6 Chrome

- **Save status:** Save now (Ctrl+S) [E] `project.save`; Autosave settings… [UI] `openSettings`; Show last saved time.
- **Export button / chip:** Export… (Ctrl+M) [E]; Cancel export (when running) [E] `job.cancel`; Show progress [UI]; Reveal file (when done) [H].
- **Workspace switcher / Reset layout button:** Reset this workspace, Reset all workspaces, Save layout as… [WP-2H.01].
- **Undo and Redo buttons:** Undo, Redo, Open History… [WP-2H.11] (replaces an undo-step list).
- **Toast:** Copy message, Copy error code, Dismiss, Dismiss all [H][UI]; "Show in History" for edit toasts [WP-2H.11].
- **Engine status chip:** Reconnect now (when disconnected), Copy diagnostics, Open logs folder [H][M] (host IPC).
- **Command palette row (2H.04):** Run, Copy command id, Assign shortcut… (opens 2H.05 at the command) [UI].
- **Agents and MCP rows (Settings):** Copy setup, Remove folder, Revoke [E] existing settings actions.

### 6.7 Planned panels (short lists; each WP owns its table)

- **Effects, Transitions, Filters card** (4A.09): Apply to selection, Add to favourites, Copy settings. **Clip effect row** (4A.09): Enable/Disable, Copy, Paste, Reset, Remove, Move up/down.
- **Mixer fader / meter** (2E.05): Reset to 0 dB, Mute, Solo, Set bus ▸.
- **Fade handle / volume line** (2E.05): Linear, Logarithmic and the other fade curves; Reset.
- **Export queue row** (2G.04): Cancel, Retry, Move up/down, Reveal file, Copy error.
- **History row** (2H.11): Go to this step, Copy label.
- **Transcript** (3D.06, 3F.03): Delete words, Cut from timeline, Copy, Add marker, Seek here. **Caption row** (3E.11): Split, Merge with next, Style…, Delete. **Style card:** Apply, Set as default.
- **Library card** (3G.05): Preview, Add to timeline, Favourite. **Text preset** (3C.05): Apply, Save current as preset.
- **AI Studio** (5F.04, 5F.05): result tile: Send to timeline, Extend clip, Fill gap, Open provenance, Copy prompt, Regenerate; job row: Cancel, Retry, Copy error.
- **Mask / handle** (4G.02): Invert, Duplicate, Delete, Feather… **Speed curve point** (4H.04): Delete point, Smooth, Corner.
- **Chat** (6I.07): Copy message, Retry, Undo this turn. **Template slot** (6D.03): Make replaceable [WP-6D.03].

### 6.8 Existing versus missing commands (summary)

| Needed by a menu | Status |
|---|---|
| `clip.split`, `item.delete`, `item.rename`, `item.set_enabled`, `item.set_props`, `item.move`, `clip.trim`, `clip.move` | Exist |
| `track.add`, `track.remove`, `track.set` (name, mute, solo, hide, lock, gain) | Exist |
| `media.import`, `media.remove`, `media.list`, `timeline.insert`, `timeline.append_clip` | Exist |
| `view.select`, `view.set`, `view.set_playhead`, `view.set_zoom`, `view.seek`, `view.play`, `view.pause`, `view.step`, `view.set_source`, `view.set_preview_size` | Exist |
| `edit.undo`, `edit.redo`, `project.save`, `export.start`, `job.cancel`, `render.frame` | Exist |
| Overwrite, lift, extract, ripple delete, delete gap, move by lanes | Planned: WP-2A.03 |
| Link, unlink, attach | Planned: WP-2A.05 |
| Copy, paste, paste attributes, duplicate | Planned: WP-2A.06 |
| Replace clip | Planned: WP-2A.11 |
| Markers, in/out, track reorder, height, magnetic | Planned: WP-2A.02 |
| Speed, reverse, freeze | Planned: WP-2D.01 |
| Extract audio, normalize, fades | Planned: WP-2E.03, WP-2E.01 |
| Keyframes, reset properties | Planned: WP-2C.03 |
| Media folders, move, rename, tags, collections, relink | Planned: WP-2F.03, 2F.06, 2F.07, 2F.08 |
| Compound clip | Planned: WP-2B.04 |
| Effects, transitions, masks, color | Planned: P4 |

### 6.9 Missing everywhere

These have no command and no WP; the plan proposes where each lands. All are small, document-changing, and so need the usual `#[command]` with MCP tag and undo test; none changes an existing schema.

| Gap | Used by | Proposed home |
|---|---|---|
| `sequence.create_from_media{media[]}`: new sequence holding the selected assets in order (Resolve's "Create timeline using selected clips") | bin card menu | WP-2B.01 (multi-sequence commands) bullet |
| `item.set_label_color{items[], color}` (color label; model field is planned) | clip menu | WP-2A.02 bullet (model) plus WP-2A.10 UI |
| `item.reset_props{items, group?|props?}`: reset properties to defaults (the Inspector section and row resets exist as buttons in `ui` with nothing behind them) | clip menu, Inspector section and row menus | WP-2C.03 bullet (it owns the property fields) |
| `timeline.update_marker{id, label?, color?, duration?}` | marker menu | WP-2A.02 bullet |
| Extend clip to next edit / trim to playhead as `to` forms | edge menu | WP-2A.04 bullet (arguments only, no new command if `clip.trim{to}` suffices) |
| `media.import_frame{at, source}`: save a Preview frame into the bin | preview menu | WP-2F.05 bullet (stills import) |
| Reveal, copy text, copy image, save frame: host actions | many | WP-2H.12 (IPC allowlist; parity-exempt list, not commands) |
| Menu choice that picks the target of "select all on track" | lane/track menus | None: built from `view.select` with ids from the snapshot |

## 7. Plan

### 7.1 New work packages

| WP | Title | Size | Executor | Depends on |
|---|---|---|---|---|
| **WP-2H.12** | Context-menu framework: host, surface registry, command-derived items, keyboard and touch, contributions | M | sonnet | E1M, E1N |
| **WP-2H.13** | Context menus for today's surfaces: bin, preview, timeline basics, inspector, chrome, text fields | M | sonnet | WP-2H.12 |

Both are in epic 2H (Workspace UI), next to the palette (2H.04) and keymap (2H.05); full text in [P2-core-editor.md](../wbs/P2-core-editor.md).

Order [D]: 2H.12 first (it only needs P1, all merged), 2H.13 straight after, both ahead of 2H.04 and 2H.05 so that the palette and keymap read the same registry. 2H.12 owns the `when` evaluator and the `UiCommandTable`; 2H.04 adds the Rust `menu`/`when` attributes and the palette on top of them (its dependency list gains 2H.12).

### 7.2 Why two WPs and not one

One WP covering the framework plus every surface would exceed the 4 agent-day limit in AGENTS.md and block on WPs that do not exist yet (clipboard, linking, speed). The framework is useful alone (the dock tab stub, text fields), and the rollout to P1 surfaces uses only commands that exist today, so it ships value immediately. Later surfaces ride on their own WPs.

### 7.3 Bullets added to existing WPs

Each bullet names the surface, the items (from section 6) and the acceptance check; it reuses WP-2H.12's API and adds an axe check on the opened menu. A flow already in `docs/user-flows.md` is extended where one exists.

- **P2:** 2A.02 (marker and track menus' commands, color label), 2A.03, 2A.04, 2A.05 (Link/Unlink in the clip menu), 2A.06 (Cut/Copy/Paste/Duplicate in clip and lane menus), 2A.10 (the full timeline menu set; replaces its one-line mention), 2A.11, 2A.14 (Source view menu), 2B.01 (create from media), 2B.03 (sequence tab), 2B.05, 2C.05 (keyframe menu), 2D.01 (Speed submenu), 2E.03, 2E.05, 2F.03, 2F.05, 2F.06, 2F.08, 2G.04, 2H.01 (dock tab and empty dock), 2H.04, 2H.05, 2H.06, 2H.07 (review checklist), 2H.11.
- **P3:** 3A.07, 3D.06, 3E.11, 3F.03, 3G.05.
- **P4:** 4A.09, 4F.06, 4G.02, 4H.04.
- **P5:** 5F.04, 5F.05.
- **P6:** 6B.03 (plugin `contributes.menus` on the registry), 6D.04.
- **P7:** 7C.01 (keyboard access and axe over every surface table).

### 7.4 Contract and ADR

- **No command schema changes** are needed by the framework itself; the gaps in 6.9 are ordinary additions inside their WPs.
- **New contract:** the surface ID list and the `when` context keys are a plugin-manifest contract (AGENTS.md: contracts change only via ADR). [ADR-0025](../adr/0025-context-menu-surfaces-and-contributions.md) records it as **Proposed**, decided by WP-2H.12 (pass criteria in the ADR) and consumed by WP-6B.03. It does not block P2.
- **Design spec:** WP-2H.12 updates [design/components/Menu/README.md](../../design/components/Menu/README.md) (submenu, disabled reason, command ids that match the real registry: its mapping uses names like `clip.rippleDelete`, which are not the registry's `timeline.ripple_delete`).

### 7.5 Risks

- **Canvas hit-testing at open time** can differ from what the pointer-down saw if a patch lands between them; the host resolves the target once on `contextmenu` and keeps ids, not indexes (ADR-0004).
- **A menu item that needs several commands** must be one composite command, or the menu will create several undo steps. 2H.12's parity test flags closures that call more than one mutating command.
- **Menu length** creeps: the rule in 5.5 and the UX review (2H.07) are the brake.
- **Keymap drift:** shortcut hints must come from `keysFor`, never from literals; the existing `shortcuts/bindings.test.ts` approach is extended to surface tables.
