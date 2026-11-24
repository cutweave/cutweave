SettingRow is one setting in the Settings dialog: its name and one-line description on the left, its control on the right, and a reset button that shows only while the value differs from its default.

**When to use**
- Every row of a settings page (File > Settings…, WP-1M.11), in a `SettingsLayout`. Later pages (Export, Keyboard shortcuts, Agents and MCP, Cache and storage) register rows; none draws its own.
- Not for the inspector (use `InspectorRow`: compact, no description) and not for a dialog's form fields (use the `.cw-row` pattern).

**Anatomy**
`.cw-setting` > `.cw-setting__text` (`.cw-setting__label` in `label`, `text-primary`; `.cw-setting__description` in `caption`, `text-muted`; optional `.cw-setting__note` in `caption`, `text-secondary`), `.cw-setting__control` (the control, at most half the row wide) and `.cw-setting__reset` (a fixed `control-sm` slot that holds the reset `IconButton`, so controls line up whether or not it shows). A `border-subtle` rule separates rows.

**The consumer provides**
`id` (also the prefix of the label's and description's ids), `label`, a one-line `description`, `htmlFor` when the control is an input (a click on the label then reaches it) or an `aria-label` on a control that has none (a segmented control), `modified` and `onReset` (the value differs from its default), and optionally `note` (the condition: "Needs the engine", "Applies at the next start"), `disabled`, `highlighted` and, for a search, `labelContent` and `descriptionContent` with the match marked.

**States**
Default; modified (the reset button appears; its name is "Reset " and the label); with a note (the condition under the description); disabled (label `text-secondary`, never `text-disabled`, so the words stay readable; the host disables the control too and says why in the note); highlighted (`marigold-subtle` wash, description in `text-secondary` for contrast: the row a deep link or a search result points at).

**Keyboard & accessibility**
The reset button is a real button with its own name and tooltip and is in the tab order only while it shows. The label names the control; the description is available to it through `aria-describedby="<id>-description"`. A disabled row keeps its label and description readable; the note says why.

**Command mapping**
None for most rows: a preference is written by main's store (`prefs:set`), not by a command. A row for a project setting (File > Project settings…, WP-2H.08) dispatches the setting's undoable command instead.

**Do**
- Say what the setting does in one line, in the person's words: "Dark, light, or follow your system".
- Put the unit next to the control ("30 s", "projects"), not in the label.
- Say the condition in the note when the control cannot act now.

**Don't**
- Don't put two controls in one row (use two rows) unless they are one value (a folder and its "Choose…").
- Don't show the reset button on a default value, and don't hide a row because its value is the default.
- Don't use marigold except for the highlighted wash and the focus edge.
