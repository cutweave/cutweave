// SPDX-License-Identifier: GPL-3.0-or-later
import { type ClipboardEvent, type KeyboardEvent, useId, useRef, useState } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface TagInputTag {
  /** The tag as shown and as handed back to `onRemove` and `onPromote`. */
  tag: string;
  /** Only some of the edited things have it (a multi-selection): drawn dashed, with `detail`. */
  partial?: boolean | undefined;
  /** Short text after a partial tag: "2 of 3". */
  detail?: string | undefined;
}

export interface TagInputProps {
  tags: readonly TagInputTag[];
  /** The text being typed. The host clears it when it accepts an `onAdd`. */
  value: string;
  onValueChange: (value: string) => void;
  /**
   * Enter, a comma or a chosen suggestion: the text to add (pasted text may hold several tags,
   * separated by commas; the host splits and checks it).
   */
  onAdd: (text: string) => void;
  onRemove: (tag: string) => void;
  /** A partial tag's "add to all" button; left out, partial tags have none. */
  onPromote?: ((tag: string) => void) | undefined;
  /** Existing tags that fit what is typed, best first. */
  suggestions?: readonly string[] | undefined;
  /** Names the text field: "Add tag". */
  label: string;
  /** Names the list of chips: "Tags". */
  tagsLabel: string;
  /** Names the suggestion list: "Suggested tags". */
  suggestionsLabel: string;
  /** Names a chip's remove button: `(tag) => "Remove tag interview"`. */
  removeLabel: (tag: string) => string;
  /** Names a partial chip's add-to-all button: `(tag) => "Add interview to all"`. */
  promoteLabel?: ((tag: string) => string) | undefined;
  placeholder?: string | undefined;
  /** What is wrong with the typed text ("A tag has no comma"); marks the field invalid. */
  error?: string | undefined;
  disabled?: boolean | undefined;
  autoFocus?: boolean | undefined;
  className?: string | undefined;
}

/**
 * Tags as removable chips with a field that adds more. Enter or a comma adds what is typed;
 * Backspace in the empty field removes the last chip; Down and Up walk the suggestions, Enter
 * takes the highlighted one and Escape puts them away. The field is a combobox over the
 * suggestion list, which sits in the flow under the field so no popover can be clipped by a
 * dialog or a narrow panel.
 */
export function TagInput({
  tags,
  value,
  onValueChange,
  onAdd,
  onRemove,
  onPromote,
  suggestions = [],
  label,
  tagsLabel,
  suggestionsLabel,
  removeLabel,
  promoteLabel,
  placeholder,
  error,
  disabled = false,
  autoFocus,
  className,
}: TagInputProps) {
  const id = useId();
  const listId = `${id}-suggestions`;
  const errorId = `${id}-error`;
  const input = useRef<HTMLInputElement>(null);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  // The highlighted suggestion, by its text so a changing list cannot point at another one.
  const [active, setActive] = useState<string | null>(null);
  const open = focused && !dismissed && !disabled && suggestions.length > 0;
  const activeIndex = open && active !== null ? suggestions.indexOf(active) : -1;

  const change = (next: string) => {
    setDismissed(false);
    setActive(null);
    onValueChange(next);
  };
  const add = (text: string) => {
    setActive(null);
    if (text.trim() !== "") onAdd(text);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    switch (event.key) {
      case "Enter":
        // Never the dialog's primary action: Enter in this field adds a tag.
        event.preventDefault();
        event.stopPropagation();
        add(activeIndex >= 0 ? (suggestions[activeIndex] ?? value) : value);
        break;
      case ",":
        event.preventDefault();
        add(value);
        break;
      case "Backspace": {
        const last = tags[tags.length - 1];
        if (value === "" && last !== undefined) {
          event.preventDefault();
          onRemove(last.tag);
        }
        break;
      }
      case "ArrowDown":
      case "ArrowUp": {
        if (suggestions.length === 0) return;
        event.preventDefault();
        setDismissed(false);
        const step = event.key === "ArrowDown" ? 1 : -1;
        const from = activeIndex < 0 ? (step > 0 ? -1 : 0) : activeIndex;
        const next = (from + step + suggestions.length) % suggestions.length;
        setActive(suggestions[next] ?? null);
        break;
      }
      case "Escape":
        // Escape puts the suggestions away and goes no further. (A dialog hears Escape before
        // the field does, so there it closes the dialog as usual.)
        if (open) {
          event.preventDefault();
          event.stopPropagation();
          setDismissed(true);
          setActive(null);
        }
        break;
      default:
        break;
    }
  };

  const onPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const text = event.clipboardData.getData("text");
    // Several tags at once: hand them over whole instead of typing a comma into the field.
    if (/[,\r\n]/.test(text)) {
      event.preventDefault();
      add(`${value}${text}`);
    }
  };

  return (
    <div className={cx("cw-taginput", className)}>
      {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: a click on the box's padding goes to the field, as in any text box */}
      <div
        className="cw-taginput__box"
        data-invalid={error === undefined ? undefined : "true"}
        data-disabled={disabled ? "true" : undefined}
        onClick={(event) => {
          if (event.target === event.currentTarget) input.current?.focus();
        }}
      >
        {tags.length === 0 ? null : (
          <ul className="cw-taginput__chips" aria-label={tagsLabel}>
            {tags.map(({ tag, partial, detail }) => (
              <li
                key={tag}
                className={cx("cw-chip", partial && "cw-chip--partial")}
                data-tag={tag}
                data-partial={partial ? "true" : undefined}
              >
                <span className="cw-chip__label label">{tag}</span>
                {detail === undefined ? null : (
                  <span className="cw-chip__detail caption">{detail}</span>
                )}
                {partial && onPromote !== undefined && promoteLabel !== undefined ? (
                  <button
                    type="button"
                    className="cw-chip__remove"
                    aria-label={promoteLabel(tag)}
                    title={promoteLabel(tag)}
                    disabled={disabled}
                    onClick={() => onPromote(tag)}
                  >
                    <Icon name="plus" size="sm" />
                  </button>
                ) : null}
                <button
                  type="button"
                  className="cw-chip__remove"
                  aria-label={removeLabel(tag)}
                  title={removeLabel(tag)}
                  disabled={disabled}
                  onClick={() => onRemove(tag)}
                >
                  <Icon name="x" size="sm" />
                </button>
              </li>
            ))}
          </ul>
        )}
        <input
          ref={input}
          type="text"
          role="combobox"
          className="cw-taginput__field body"
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          spellCheck={false}
          autoComplete="off"
          // biome-ignore lint/a11y/noAutofocus: a dialog opened to add a tag starts in its field
          autoFocus={autoFocus}
          aria-label={label}
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-activedescendant={activeIndex >= 0 ? `${listId}-${activeIndex}` : undefined}
          aria-invalid={error === undefined ? undefined : "true"}
          aria-describedby={error === undefined ? undefined : errorId}
          onChange={(event) => change(event.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onFocus={() => {
            setFocused(true);
            setDismissed(false);
          }}
          onBlur={() => {
            setFocused(false);
            setActive(null);
          }}
        />
      </div>
      {error === undefined ? null : (
        <p id={errorId} className="cw-taginput__error caption" role="alert">
          {error}
        </p>
      )}
      {open ? (
        <div
          id={listId}
          role="listbox"
          aria-label={suggestionsLabel}
          className="cw-taginput__suggestions"
        >
          {suggestions.map((tag, i) => (
            // biome-ignore lint/a11y/useKeyWithClickEvents lint/a11y/useFocusableInteractive: the keys are the combobox's (Down, Up, Enter)
            <div
              key={tag}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === activeIndex}
              className="cw-taginput__option label"
              // The field keeps the focus, so the list stays open for the click.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => add(tag)}
            >
              {tag}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
