Tabs switch between sibling views inside one panel; the active tab gets a marigold label and a 2px marigold underline.

**When to use**
- The asset browser (Media, Audio, Text, Stickers, Effects, Transitions, Captions, AI Studio).
- Inspector groupings (Video, Audio, Speed) and any panel with parallel views.
- Not for choosing a setting value (use SegmentedControl).

**Anatomy**
`.cw-tabs` (`panel-header` tall, `space-1` gap, `border-subtle` bottom rule, horizontal scroll on overflow) > `.cw-tabs__tab` (type: label, `space-2` padding, optional `icon-md` icon with `space-1-5` gap). Inside a Panel header the bottom border is removed and the strip fills the header height.

**The consumer provides**
Tab ids and labels, the selected id, optional icons (AI Studio uses sparkles), and the panel content for each tab.

**States**
Default (`text-secondary`), hover (`surface-3`, `text-primary`), selected (`aria-selected="true"`: `marigold-text` label + 2px `marigold` underline), focus-visible, disabled (`text-disabled`).

**Keyboard & accessibility**
`role="tablist"` / `role="tab"` / `role="tabpanel"`. Left/Right move between tabs (roving tabindex), Home/End jump; activation follows focus. Selected state is shown by underline and color together. Overflow scrolls horizontally and keeps the selected tab in view.

**Command mapping**
Selecting a tab dispatches `layout.panel.setTab {panel:"assets", tab:"captions"}`; opening AI Studio from the command palette uses the same command.

**Do**
- Keep labels to one word where possible; tab text is `label` type, not `heading`.
- Use `marigold-text` (not `marigold`) for the label so it passes contrast in the light theme.
- Keep a consistent order across sessions; do not reorder by recent use.

**Don't**
- Don't fill the active tab with `marigold-subtle`; the underline is the indicator.
- Don't wrap tabs onto a second row; scroll instead.
- Don't use agent blue for the AI Studio tab unless it holds an unread agent proposal.
