// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's toolbar (WP-2A.10): the tools, Split, markers and in/out, the snapping and
// linked-selection toggles, and zoom. Icons with tooltips that name the action and its key from
// the live keymap; every button goes through the timeline host, as its key does. Memoized on the
// few values it shows (issue #241): a drag re-renders the panel around it without redrawing it.
import type { Tool } from "@cutweave/timeline";
import { IconButton, type IconName, Toolbar, ToolbarSeparator, Tooltip } from "@cutweave/ui";
import { memo, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { comboText, strokesOf } from "../keymap/store.ts";
import { TIMELINE_COMMANDS } from "../keymap/timeline.ts";
import { usePref } from "../prefs/prefsStore.ts";
import { useViewStore } from "../state/viewStore.ts";
import type { TimelineHost, TimelineUi } from "./host.ts";

interface ToolDef {
  readonly tool: Tool;
  readonly icon: IconName;
  readonly label: TKey;
}

/** The tools in the order the toolbar shows them. */
export const TOOL_BUTTONS: readonly ToolDef[] = [
  { tool: "select", icon: "mouse-pointer-2", label: "timeline.select" },
  { tool: "ripple", icon: "ripple", label: "timeline.ripple" },
  { tool: "roll", icon: "roll", label: "timeline.roll" },
  { tool: "slip", icon: "slip", label: "timeline.slip" },
  { tool: "slide", icon: "slide", label: "timeline.slide" },
  { tool: "razor", icon: "razor", label: "timeline.razor" },
  { tool: "hand", icon: "hand", label: "timeline.hand" },
  { tool: "zoom", icon: "search", label: "timeline.zoomTool" },
];

/** The keys of a keymap command, for a tooltip. */
const keysOf = (command: string): string[] => strokesOf(command).map((c) => comboText(c));

function Button({
  icon,
  label,
  command,
  pressed,
  disabled,
  onClick,
}: {
  readonly icon: IconName;
  readonly label: string;
  readonly command?: string;
  readonly pressed?: boolean;
  readonly disabled: boolean;
  readonly onClick: () => void;
}): ReactElement {
  return (
    <Tooltip label={label} keys={command === undefined ? undefined : keysOf(command)}>
      <IconButton
        icon={icon}
        label={label}
        disabled={disabled}
        onClick={onClick}
        {...(pressed !== undefined && { pressed })}
      />
    </Tooltip>
  );
}

export const TimelineToolbar = memo(function TimelineToolbar({
  host,
  tool,
  linkedSelection,
  live,
}: {
  readonly host: TimelineHost;
  readonly tool: TimelineUi["tool"];
  readonly linkedSelection: boolean;
  /** An engine sequence is shown: the buttons act. */
  readonly live: boolean;
}): ReactElement {
  const { t } = useTranslation();
  const snapping = useViewStore((s) => s.snapping);
  // The tooltips name each action's keys: a rebinding in Settings shows at once.
  usePref("shortcuts.preset");
  usePref("shortcuts.bindings");
  const off = !live;
  return (
    <Toolbar aria-label={t("timeline.toolbarLabel")} className="cw-timeline__tools">
      {TOOL_BUTTONS.map((def) => (
        <Button
          key={def.tool}
          icon={def.icon}
          label={t(def.label)}
          command={TIMELINE_COMMANDS[`tool.${def.tool}`]}
          pressed={tool === def.tool}
          disabled={off}
          onClick={() => host.setTool(def.tool)}
        />
      ))}
      <ToolbarSeparator />
      <Button
        icon="scissors"
        label={t("timeline.split")}
        command="clip.split"
        disabled={off}
        onClick={() => host.split()}
      />
      <Button
        icon="bookmark"
        label={t("timeline.addMarker")}
        command="timeline.add_marker"
        disabled={off}
        onClick={() => host.addMarker()}
      />
      <Button
        icon="mark-in"
        label={t("timeline.markIn")}
        command="timeline.mark_in"
        disabled={off}
        onClick={() => host.markIn()}
      />
      <Button
        icon="mark-out"
        label={t("timeline.markOut")}
        command="timeline.mark_out"
        disabled={off}
        onClick={() => host.markOut()}
      />
      <ToolbarSeparator />
      <Button
        icon="magnet"
        label={t("timeline.snapping")}
        command="timeline.toggle_snapping"
        pressed={snapping}
        disabled={off}
        onClick={() => host.toggleSnapping()}
      />
      <Button
        icon="link"
        label={t("timeline.linkedSelection")}
        pressed={linkedSelection}
        disabled={off}
        onClick={() => host.toggleLinkedSelection()}
      />
      <ToolbarSeparator />
      <Button
        icon="zoom-out"
        label={t("timeline.zoomOut")}
        disabled={off}
        onClick={() => host.zoomBy(0.8)}
      />
      <Button
        icon="zoom-in"
        label={t("timeline.zoomIn")}
        disabled={off}
        onClick={() => host.zoomBy(1.25)}
      />
      <Button
        icon="arrow-left-right"
        label={t("timeline.zoomToFit")}
        disabled={off}
        onClick={() => host.zoomToFit()}
      />
      <Button
        icon="maximize-2"
        label={t("timeline.zoomToSelection")}
        disabled={off}
        onClick={() => host.zoomToSelection()}
      />
    </Toolbar>
  );
});
