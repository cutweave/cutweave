// SPDX-License-Identifier: GPL-3.0-or-later
import * as RadixToast from "@radix-ui/react-toast";
import type { ReactNode, Ref } from "react";
import { Icon, type IconName } from "../icons/index.ts";
import { Button } from "./Button.tsx";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";
import { ProgressBar } from "./ProgressBar.tsx";

export type ToastTone = "success" | "info" | "warning" | "danger" | "job";

const TONE_ICON: Record<ToastTone, IconName> = {
  success: "check",
  info: "info",
  warning: "triangle-alert",
  danger: "circle-alert",
  job: "download",
};

/** Success and info dismiss themselves; warnings, errors and running jobs wait for a person. */
const AUTO_DISMISS_MS: Record<ToastTone, number> = {
  success: 5000,
  info: 5000,
  warning: Number.POSITIVE_INFINITY,
  danger: Number.POSITIVE_INFINITY,
  job: Number.POSITIVE_INFINITY,
};

export interface ToastProviderProps {
  children: ReactNode;
  /** Landmark name for the viewport; F8 moves focus into it. */
  label?: string | undefined;
}

/** Mount once near the app root. Renders the fixed viewport that toasts stack in. */
export function ToastProvider({ children, label }: ToastProviderProps) {
  return (
    <RadixToast.Provider swipeDirection="right" {...(label ? { label } : {})}>
      {children}
      <RadixToast.Viewport className="cw-toast-viewport" />
    </RadixToast.Provider>
  );
}

export interface ToastAction {
  label: string;
  /** Dispatches the command (`job.retry`, `fs.reveal`, ...). The toast closes afterwards. */
  onClick: () => void;
  /** `ghost` for secondary actions; keep one primary action. */
  variant?: "secondary" | "ghost" | undefined;
}

export interface ToastProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tone: ToastTone;
  /** "Export complete", "Generation failed". */
  title: string;
  /** Real values: file names and timecodes. */
  message?: ReactNode;
  actions?: readonly ToastAction[] | undefined;
  /** Job variant: 0 to 100 with a progressbar. Update in place, then switch `tone` when done. */
  progress?: number | undefined;
  /** Job variant: the button at the right cancels the job instead of dismissing the toast. */
  onCancel?: (() => void) | undefined;
  /** Accessible name of the cancel button, e.g. "Cancel export". */
  cancelLabel?: string | undefined;
  /** Milliseconds before auto-dismiss; defaults by tone. Hover or focus pauses it. */
  duration?: number | undefined;
  className?: string | undefined;
  /** The toast's element, for a host that attaches behavior to it (the editor's context menu). */
  rootRef?: Ref<HTMLLIElement> | undefined;
}

/**
 * Transient outcome or job progress. Errors and warnings are announced assertively via Radix `type="foreground"`, others politely; no extra `role`, so nothing is announced twice.
 * Escape dismisses the focused toast. Must render inside a `ToastProvider`.
 */
export function Toast({
  open,
  onOpenChange,
  tone,
  title,
  message,
  actions,
  progress,
  onCancel,
  cancelLabel = "Cancel job",
  duration,
  className,
  rootRef,
}: ToastProps) {
  const showCancel = tone === "job" && onCancel !== undefined;
  return (
    <RadixToast.Root
      ref={rootRef}
      open={open}
      onOpenChange={onOpenChange}
      duration={duration ?? AUTO_DISMISS_MS[tone]}
      type={tone === "danger" || tone === "warning" ? "foreground" : "background"}
      className={cx("cw-toast", className)}
      data-tone={tone}
    >
      <Icon name={TONE_ICON[tone]} size="md" className="cw-toast__icon" />
      <div className="cw-toast__main">
        <RadixToast.Title className="cw-toast__title body-strong">{title}</RadixToast.Title>
        {tone === "job" && progress !== undefined ? (
          <ProgressBar value={progress} label={title} />
        ) : null}
        {message ? (
          <RadixToast.Description className="cw-toast__text body">{message}</RadixToast.Description>
        ) : null}
        {actions && actions.length > 0 ? (
          <div className="cw-toast__actions">
            {actions.map((action) => (
              <RadixToast.Action key={action.label} altText={action.label} asChild>
                <Button size="sm" variant={action.variant ?? "secondary"} onClick={action.onClick}>
                  {action.label}
                </Button>
              </RadixToast.Action>
            ))}
          </div>
        ) : null}
      </div>
      {showCancel ? (
        <IconButton size="sm" icon="x" label={cancelLabel} onClick={onCancel} />
      ) : (
        <RadixToast.Close asChild>
          <IconButton size="sm" icon="x" label="Dismiss" />
        </RadixToast.Close>
      )}
    </RadixToast.Root>
  );
}
