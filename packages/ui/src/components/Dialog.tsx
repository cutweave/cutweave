// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixDialog from "@radix-ui/react-dialog";
import { type KeyboardEvent, type ReactNode, useRef } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { PortalContainerProvider } from "./PortalContainer.ts";

const FOCUSABLE =
  'input:not([disabled]):not([type="hidden"]), textarea:not([disabled]), select:not([disabled]), button:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';

/**
 * A dialog takes the focus of the window it opens in: an app dialog raised while the person works
 * in a detached panel's window brings the main window forward (WP-2H.15).
 */
function focusWindowOf(doc: Document): void {
  try {
    if (!doc.hasFocus()) doc.defaultView?.focus();
  } catch {
    // A window that is going away keeps whatever focus it has.
  }
}

/** Whether `doc` is the main document: the one Radix places and focuses dialogs in by default. */
function isMainDocument(doc: Document): boolean {
  // biome-ignore lint/style/noRestrictedGlobals: Radix's own default is the main document
  return doc === document;
}

/**
 * Keeps Tab inside `content` when it is in another window's document: Radix's focus scope reads
 * the main document's focus, so it cannot wrap the ends there (WP-2H.15). Returns whether it
 * handled the key.
 */
export function trapTab(event: KeyboardEvent<HTMLElement>, content: HTMLElement): boolean {
  if (event.key !== "Tab" || event.altKey || event.ctrlKey || event.metaKey) return false;
  const doc = content.ownerDocument;
  if (isMainDocument(doc)) return false;
  const stops = [...content.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => !el.hasAttribute("hidden") && el.getAttribute("aria-hidden") !== "true",
  );
  event.preventDefault();
  const first = stops[0];
  const last = stops[stops.length - 1];
  if (first === undefined || last === undefined) {
    content.focus();
    return true;
  }
  const at = stops.indexOf(doc.activeElement as HTMLElement);
  if (event.shiftKey) (at <= 0 ? last : (stops[at - 1] ?? last)).focus();
  else (at < 0 || at === stops.length - 1 ? first : (stops[at + 1] ?? first)).focus();
  return true;
}

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
  /**
   * Where it opens: a window's `<body>` (the command palette opens in the window the person is
   * in). Left out, it opens in the main document, as app dialogs do. Menus and tooltips inside
   * it open in the same place.
   */
  container?: HTMLElement | null | undefined;
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
  container,
}: DialogProps) {
  const openerRef = useRef<HTMLElement | null>(null);
  const guard = (event: Event) => {
    if (!dirty) return;
    event.preventDefault();
    onDismissAttempt?.();
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (trapTab(event, event.currentTarget)) return;
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
      <RadixDialog.Portal container={container ?? undefined}>
        <RadixDialog.Overlay className="cw-scrim" />
        <RadixDialog.Content
          aria-modal="true"
          {...(description ? {} : { "aria-describedby": undefined })}
          className={cx("cw-dialog", className)}
          onOpenAutoFocus={(event) => {
            const content = event.target as HTMLElement | null;
            const doc = content?.ownerDocument;
            if (content === null || doc === undefined) return;
            openerRef.current = doc.activeElement as HTMLElement | null;
            focusWindowOf(doc);
            // Radix finds the first field by the main document's focus; in another window's
            // document the dialog focuses it itself.
            if (!isMainDocument(doc)) {
              event.preventDefault();
              (content.querySelector<HTMLElement>(FOCUSABLE) ?? content).focus();
            }
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
          <PortalContainerProvider container={container ?? null}>
            {children}
            {footer ? <div className="cw-dialog__footer">{footer}</div> : null}
          </PortalContainerProvider>
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
