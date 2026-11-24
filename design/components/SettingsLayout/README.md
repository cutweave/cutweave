SettingsLayout is the frame of the Settings dialog: a search field over a vertical list of pages on the left, and the open page (its heading, its "Reset page" and its rows) on the right.

**When to use**
- File > Settings… (Ctrl+, / ⌘,), in a large `Dialog` with `className="cw-dialog--settings"`. One dialog for every page; later work packages add pages through the editor's page registry and never draw their own frame.
- Not for a two- or three-field dialog (use `Dialog` with `.cw-row` rows) and not for a panel's own options (use `InspectorSection`).

**Anatomy**
`.cw-settings` > `.cw-settings__side` (the `SearchField`, then `.cw-settings__nav`, a vertical tablist) and `.cw-settings__panel` (`.cw-settings__head` with the heading in `title` and the page's actions, then `.cw-settings__rows`). The nav holds groups: an optional `overline` heading ("Preferences", "Advanced") over `.cw-settings__tab` items, each an `icon-md` icon and the page name. The dialog is `layer-width-lg` plus `layer-width-sm` wide and the window height less `space-6` on each side four times; the panel scrolls, the page list scrolls, the dialog does not.
While a search shows results the panel is a named region, no page is selected, the heading reads "Results for “x”", and `SettingsResultGroup`s (an `overline` heading and the rows) group the matching `SettingRow`s by page, with the match in `<mark>` (`marigold-subtle`). With no match the panel holds one `body` line: No settings match “x”.

**The consumer provides**
`navLabel`, the `search` node (a `SearchField` labelled "Search settings", focused on open), `groups` of pages (`id`, `label`, optional `icon`), `activeId` (`null` while searching), `onSelect`, `heading`, optional `actions`, `resultsLabel` and the rows as children.

**States**
A page tab: default, hover (`surface-3`), selected (`marigold-subtle` wash, `marigold-text`; selection is shown by the wash and the colour together, not colour alone), focus-visible (the shared focus ring). Searching: no tab is selected.

**Keyboard & accessibility**
The page list is a vertical `tablist`: Up and Down move between pages and open them, Home and End jump to the first and last, Tab leaves the list for the page. The page is the `tabpanel` of its tab. The search field follows SearchField: Esc clears the text first and the open page comes back; on an empty field it hands focus to the page list, and the next Esc closes the dialog. The search field is focused when the dialog opens. A result count is announced politely by the field.

**Command mapping**
Opening is the `settings` app command (`openSettings(page, key?)` goes to a page and a row). Nothing here is an engine command: the rows write preferences through main's store, and project settings have their own dialog.

**Do**
- Group pages as the catalog does (docs/reference/settings.md) and show a page only once one of its rows has shipped.
- Name pages in the person's words ("Saving and backups"), one noun phrase each.
- Keep the page's heading and its "Reset page" in the head, and nothing else.

**Don't**
- Don't nest tabs or sections inside a page: one level of pages, rows below.
- Don't make the dialog resizable or add a second search.
- Don't use marigold except for the selected page, the matches and the focus edge.
