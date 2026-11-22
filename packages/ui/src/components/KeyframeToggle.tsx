// SPDX-License-Identifier: GPL-3.0-or-later
import { forwardRef } from "react";
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";
import { IconButton } from "./IconButton.tsx";

/**
 * `off`: no keyframes; `on`: a keyframe exists exactly at the playhead;
 * `elsewhere`: keyframes exist at other times only.
 */
export type KeyframeState = "off" | "on" | "elsewhere";

export interface KeyframeToggleProps {
  state: KeyframeState;
  onToggle: () => void;
  /** Property name, appended to the accessible name ("Add keyframe for Opacity"). */
  property?: string | undefined;
  /** Property cannot animate; the slot stays reserved. */
  disabled?: boolean | undefined;
  className?: string | undefined;
}

function keyframeLabel(state: KeyframeState, disabled: boolean, property?: string): string {
  if (disabled) return "Keyframes unavailable";
  const base =
    state === "on"
      ? "Remove keyframe at playhead"
      : state === "elsewhere"
        ? "Add keyframe at playhead"
        : "Add keyframe";
  return property ? `${base} for ${property}` : base;
}

export const KeyframeToggle = forwardRef<HTMLButtonElement, KeyframeToggleProps>(
  function KeyframeToggle({ state, onToggle, property, disabled = false, className }, ref) {
    return (
      <button
        ref={ref}
        type="button"
        className={cx("cw-kf", className)}
        data-state={state}
        aria-pressed={state === "on"}
        aria-label={keyframeLabel(state, disabled, property)}
        disabled={disabled}
        onClick={onToggle}
      >
        <Icon name={state === "on" ? "keyframe-diamond-filled" : "keyframe-diamond"} />
      </button>
    );
  },
);

export interface KeyframeGroupProps extends KeyframeToggleProps {
  onPrev?: (() => void) | undefined;
  onNext?: (() => void) | undefined;
  prevDisabled?: boolean | undefined;
  nextDisabled?: boolean | undefined;
}

/** Previous / toggle / next keyframe controls for an animated property. */
export function KeyframeGroup({
  onPrev,
  onNext,
  prevDisabled,
  nextDisabled,
  ...toggle
}: KeyframeGroupProps) {
  const name = toggle.property ? ` for ${toggle.property}` : "";
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
    <div role="group" aria-label={`Keyframes${name}`} className="cw-kfgroup">
      {onPrev ? (
        <IconButton
          size="sm"
          icon="chevron-left"
          label="Previous keyframe (Alt+Left)"
          onClick={onPrev}
          disabled={prevDisabled || toggle.disabled}
        />
      ) : null}
      <KeyframeToggle {...toggle} />
      {onNext ? (
        <IconButton
          size="sm"
          icon="chevron-right"
          label="Next keyframe (Alt+Right)"
          onClick={onNext}
          disabled={nextDisabled || toggle.disabled}
        />
      ) : null}
    </div>
  );
}
