// SPDX-License-Identifier: GPL-3.0-or-later
import { type KeyboardEvent, useEffect, useRef } from "react";
import { cx } from "./cx.ts";

export interface InlineRenameProps {
  /** The name being edited; it is selected when the field opens. */
  value: string;
  /** Names the field for assistive technology: "Folder name". */
  label: string;
  /** Enter or leaving the field. The host decides whether an unchanged or blank name does anything. */
  onCommit: (value: string) => void;
  /** Escape. */
  onCancel: () => void;
  className?: string | undefined;
}

/**
 * A name edited where it is shown (a tile's caption, a list row). Enter or blur commits, Escape
 * cancels; the keys and the pointer stay inside the field, so the grid around it does not react.
 */
export function InlineRename({ value, label, onCommit, onCancel, className }: InlineRenameProps) {
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);

  const finish = (commit: boolean) => {
    if (done.current) return;
    done.current = true;
    if (commit) onCommit(ref.current?.value ?? value);
    else onCancel();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    event.stopPropagation();
    if (event.key === "Enter") {
      event.preventDefault();
      finish(true);
    } else if (event.key === "Escape") {
      event.preventDefault();
      finish(false);
    }
  };
  return (
    <input
      ref={ref}
      type="text"
      defaultValue={value}
      aria-label={label}
      spellCheck={false}
      autoComplete="off"
      draggable={false}
      className={cx("cw-inline-rename", "body", className)}
      onKeyDown={onKeyDown}
      onBlur={() => finish(true)}
      onClick={(event) => event.stopPropagation()}
      onDoubleClick={(event) => event.stopPropagation()}
      onPointerDown={(event) => event.stopPropagation()}
    />
  );
}
