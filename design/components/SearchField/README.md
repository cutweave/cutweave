SearchField filters a list as you type: a magnifier, the text, an optional result count and a clear button.

**When to use**
- A list that people narrow by typing and that updates live: the media bin ("Search media"), the Settings dialog ("Search settings"), later the command palette's list pages.
- One field per list, at the top of the panel it filters.
- Not for a command box (use the CommandPalette), a form value (use TextField) or a search that needs a Submit.

**Anatomy**
`.cw-search` > `.cw-search__icon` (magnifier, `icon-md`, `text-muted`), `input.cw-field.cw-search__input` (`type="search"`, `control-sm` tall like every field), `.cw-search__count` (caption, `text-muted`, tabular digits) and `.cw-search__clear` (an `IconButton` `sm` with an x, shown once there is text). The field reserves room for the count and the clear button only while they show. Active filters sit under the field as `.cw-chip` pills (label and a remove button) with a text button "Clear all" after the last one.

**The consumer provides**
`label` (the accessible name, also the placeholder: "Search media"), `clearLabel` ("Clear search"), the value and `onChange` (every keystroke; no debounce is needed when filtering is local), and optionally `count` ("12 of 340"), `onArrowDown` and `onExit`.

**States**
Empty (placeholder `text-muted`), with text (clear button), focus-visible (`marigold` edge and `focus-ring`), hover (`surface-3`), disabled (`text-disabled`, `border-subtle`). The count turns up with the first character; with no match the host shows its own empty state ("No media matches "x"" with "Clear search"), not the field.

**Keyboard & accessibility**
Esc clears the text and keeps focus; Esc on an empty field leaves it (the host moves focus on, default blur). Arrow Down moves into the results. The clear button is a real button with its own name and tooltip; clicking it clears and returns focus to the field. The count is a polite live region, so a screen reader hears "2 of 14" as you type. The host may focus the field from a shortcut (Ctrl or Cmd+F while its panel has focus).

**Command mapping**
For the media bin the same words go to `media.list {filter: {text}}`, so an agent sees exactly the list a person sees. Settings search has no command: it filters the page registry.

**Do**
- Say what is searched in the label ("Search media", "Search settings"), never just "Search".
- Show how many results there are as a count, and keep the selection when a filter hides it.
- Match the words in any order and ignore case and accents.

**Don't**
- Don't add a submit button or require Enter.
- Don't clear the field when the list changes underneath it.
- Don't use marigold for the typed text; marigold is the focus edge only.
