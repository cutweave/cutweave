// SPDX-License-Identifier: GPL-3.0-or-later
import { Plug, User } from "lucide-react";
import { ICON_SIZE_TOKENS, ICON_STROKE_WIDTH, Icon } from "../icons/index.ts";
import { cx } from "./cx.ts";

export type ActorKind = "user" | "agent" | "plugin";

export interface ActorBadgeProps {
  kind: ActorKind;
  /** Agent client or plugin id shown after the colon ("claude-code", "acme.lut-pack"). Ignored for `user`. */
  id?: string | undefined;
  className?: string | undefined;
}

// `user` and `plug` are not in the shared Lucide subset yet; same grid, stroke and size tokens as <Icon>.
function GlyphIcon({ glyph: Glyph }: { glyph: typeof User }) {
  return (
    <Glyph
      aria-hidden="true"
      focusable="false"
      strokeWidth={ICON_STROKE_WIDTH}
      style={{ width: ICON_SIZE_TOKENS.sm, height: ICON_SIZE_TOKENS.sm, flexShrink: 0 }}
    />
  );
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
      {kind === "agent" ? (
        <Icon name="sparkles" size="sm" />
      ) : (
        <GlyphIcon glyph={kind === "plugin" ? Plug : User} />
      )}
      {text}
    </span>
  );
}
