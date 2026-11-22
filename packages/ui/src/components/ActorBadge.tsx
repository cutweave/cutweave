// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

export type ActorKind = "user" | "agent" | "plugin";

export interface ActorBadgeProps {
  kind: ActorKind;
  /** Agent client or plugin id shown after the colon ("claude-code", "acme.lut-pack"). Ignored for `user`. */
  id?: string | undefined;
  className?: string | undefined;
}

/** Who made an edit. The text is always present, so the icon is decorative. */
export function ActorBadge({ kind, id, className }: ActorBadgeProps) {
  const text =
    kind === "user" ? "You" : `${kind === "agent" ? "Agent" : "Plugin"}${id ? `: ${id}` : ""}`;
  return (
    <span
      className={cx("cw-actor caption", kind !== "user" && `cw-actor--${kind}`, className)}
      data-actor={kind}
    >
      <Icon name={kind === "agent" ? "sparkles" : kind === "plugin" ? "plug" : "user"} size="sm" />
      {text}
    </span>
  );
}
