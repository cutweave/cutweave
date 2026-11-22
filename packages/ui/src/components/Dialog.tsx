// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixDialog from "@radix-ui/react-dialog";
import { type KeyboardEvent, type ReactNode, useRef } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";

export interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Names the dialog (`aria-labelledby`): "Export video". */
  title: string;
  /** Optional one-line purpose, wired to `aria-describedby`. */
  description?: string | undefined;
  /** Field rows (`.cw-row`), notes and validation. Keep to one scrollable region. */
  children: ReactNode;
  /** Cancel (ghost) and the single emphasized action, both `size="lg"`. */
  footer?: ReactNode;
  /** The opener; focus returns to it on close. Omit when `open` is driven elsewhere. */
  trigger?: ReactNode;
  /** Runs the primary action on Enter, except from multi-line fields and buttons. */
  onSubmit?: (() => void) | undefined;
  /** Unsaved changes: Escape and outside clicks do not close; `onDismissAttempt` fires instead. */
  dirty?: boolean | undefined;
  onDismissAttempt?: (() => void) | undefined;
  className?: string | undefined;
}

/** Modal sheet on `surface-2` over a scrim. Focus is trapped and restored by the host (Radix). */
export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  footer,
  trigger,
  onSubmit,
  dirty,
  onDismissAttempt,
  className,
}: DialogProps) {
  const openerRef = useRef<HTMLElement | null>(null);
  const guard = (event: Event) => {
    if (!dirty) return;
    event.preventDefault();
    onDismissAttempt?.();
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (
      event.key !== "Enter" ||
      !onSubmit ||
      event.defaultPrevented ||
      event.nativeEvent.isComposing
    )
      return;
    const target = event.target as HTMLElement;
    if (target.closest("textarea, select, button, a, [contenteditable='true']")) return;
    event.preventDefault();
    onSubmit();
  };
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger ? <RadixDialog.Trigger asChild>{trigger}</RadixDialog.Trigger> : null}
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="cw-scrim" />
        <RadixDialog.Content
          aria-modal="true"
          {...(description ? {} : { "aria-describedby": undefined })}
          className={cx("cw-dialog", className)}
          onOpenAutoFocus={() => {
            openerRef.current = document.activeElement as HTMLElement | null;
          }}
          onCloseAutoFocus={(event) => {
            // Radix only restores focus to its own Trigger; cover dialogs opened by app state.
            if (trigger) return;
            event.preventDefault();
            openerRef.current?.focus();
          }}
          onKeyDown={handleKeyDown}
          onEscapeKeyDown={guard}
          onPointerDownOutside={guard}
          onInteractOutside={guard}
        >
          <RadixDialog.Title className="cw-dialog__title title">{title}</RadixDialog.Title>
          {description ? (
            <RadixDialog.Description className="cw-note">{description}</RadixDialog.Description>
          ) : null}
          {children}
          {footer ? <div className="cw-dialog__footer">{footer}</div> : null}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

export interface DialogNoteProps {
  icon?: IconName | undefined;
  children: ReactNode;
}

/** Cost or info line (`text-muted`, with icon). Say the cost even when it is free. */
export function DialogNote({ icon = "info", children }: DialogNoteProps) {
  return (
    <p className="cw-note">
      <Icon name={icon} size="md" />
      <span>{children}</span>
    </p>
  );
}
