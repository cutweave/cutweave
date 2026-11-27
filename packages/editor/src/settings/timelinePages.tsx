// SPDX-License-Identifier: GPL-3.0-or-later
// Settings > Editing (its editing defaults) and Settings > Timeline (WP-2A.13). All rows are app
// preferences: the timeline, the transport and the Inspector read them (`timeline/prefs.ts`), and
// the undo steps go to the engine (`edit.set_history_limit`, `engine/runtime.ts`). Nothing here is
// written to a project. The Editing page itself is `featurePages.tsx`'s: these rows join it.

import {
  DROP_MODES,
  FOLLOW_MODES,
  NUDGE_MAX,
  SNAP_DISTANCE_MAX,
  SNAP_DISTANCE_MIN,
  SNAP_TARGETS,
  type SnapTarget,
  THUMBNAIL_MODES,
  TIME_DISPLAYS,
  TRACK_HEIGHTS,
  UNDO_STEPS_MAX,
  WHEEL_MODES,
} from "@cutweave/preferences";
import { Switch } from "@cutweave/ui";
import type { CSSProperties } from "react";
import type { TKey } from "../i18n/index.ts";
import {
  type ChoiceOption,
  registerSettingRow,
  registerSettingsPage,
  type SettingRowDef,
  type SettingsContext,
  type SettingsPageDef,
} from "./registry.ts";

const choices = <T extends string>(
  values: readonly T[],
  labels: Readonly<Record<T, NonNullable<ChoiceOption["label"]>>>,
): readonly ChoiceOption[] => values.map((value) => ({ value, label: labels[value] }));

// ---- Editing ----------------------------------------------------------------------------------

/** The rows this work package adds to the Editing page. */
export const EDITING_ROWS: readonly SettingRowDef[] = [
  {
    id: "editing.link_av",
    key: "editing.link_av",
    label: "settings.rows.linkAv.label",
    description: "settings.rows.linkAv.description",
    keywords: "settings.rows.linkAv.keywords",
    control: { kind: "switch" },
  },
  {
    id: "editing.drop_mode",
    key: "editing.drop_mode",
    label: "settings.rows.dropMode.label",
    description: "settings.rows.dropMode.description",
    keywords: "settings.rows.dropMode.keywords",
    control: {
      kind: "segmented",
      options: choices(DROP_MODES, {
        overwrite: "settings.rows.dropMode.overwrite",
        insert: "settings.rows.dropMode.insert",
      }),
    },
  },
  {
    id: "editing.nudge",
    key: "editing.nudge",
    label: "settings.rows.nudge.label",
    description: "settings.rows.nudge.description",
    keywords: "settings.rows.nudge.keywords",
    control: { kind: "number", min: 1, max: NUDGE_MAX, unit: "settings.rows.nudge.unit" },
  },
  {
    id: "editing.undo_steps",
    key: "editing.undo_steps",
    label: "settings.rows.undoSteps.label",
    description: "settings.rows.undoSteps.description",
    keywords: "settings.rows.undoSteps.keywords",
    control: { kind: "number", min: 1, max: UNDO_STEPS_MAX, unit: "settings.rows.undoSteps.unit" },
    applies: "engine",
  },
];

// ---- Timeline ---------------------------------------------------------------------------------

const SNAP_TARGET_LABELS: Readonly<Record<SnapTarget, TKey>> = {
  playhead: "settings.rows.snapTargets.playhead",
  edges: "settings.rows.snapTargets.edges",
  markers: "settings.rows.snapTargets.markers",
  beats: "settings.rows.snapTargets.beats",
};

const TARGET_LIST: CSSProperties = {
  // A fieldset without its frame: the row already names the group.
  border: 0,
  margin: 0,
  padding: 0,
  minWidth: 0,
  display: "flex",
  flexWrap: "wrap",
  justifyContent: "flex-end",
  gap: "var(--space-2) var(--space-4)",
};
const TARGET: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  gap: "var(--space-2)",
};

/** "Snap to": one switch per kind of thing a dragged clip can snap to. */
function SnapTargetsControl({ ctx, id }: { readonly ctx: SettingsContext; readonly id: string }) {
  const on = ctx.values["timeline.snap_targets"];
  const toggle = (target: SnapTarget, checked: boolean): void => {
    const next = SNAP_TARGETS.filter((t) => (t === target ? checked : on.includes(t)));
    void ctx.client.set("timeline.snap_targets", next);
  };
  return (
    <fieldset id={id} aria-label={ctx.t("settings.rows.snapTargets.label")} style={TARGET_LIST}>
      {SNAP_TARGETS.map((target) => {
        const label = ctx.t(SNAP_TARGET_LABELS[target]);
        return (
          <span key={target} style={TARGET}>
            <span className="label" id={`${id}-${target}`}>
              {label}
            </span>
            <Switch
              checked={on.includes(target)}
              aria-labelledby={`${id}-${target}`}
              onCheckedChange={(checked) => toggle(target, checked)}
            />
          </span>
        );
      })}
    </fieldset>
  );
}

const timelinePage: SettingsPageDef = {
  id: "timeline",
  title: "settings.pages.timeline",
  icon: "layers",
  group: "preferences",
  order: 40,
  rows: [
    {
      id: "timeline.snap",
      key: "timeline.snap",
      label: "settings.rows.snap.label",
      description: "settings.rows.snap.description",
      keywords: "settings.rows.snap.keywords",
      control: { kind: "switch" },
    },
    {
      id: "timeline.snap_targets",
      key: "timeline.snap_targets",
      label: "settings.rows.snapTargets.label",
      description: "settings.rows.snapTargets.description",
      keywords: "settings.rows.snapTargets.keywords",
      control: { kind: "custom", render: (ctx, id) => <SnapTargetsControl ctx={ctx} id={id} /> },
    },
    {
      id: "timeline.snap_distance",
      key: "timeline.snap_distance",
      label: "settings.rows.snapDistance.label",
      description: "settings.rows.snapDistance.description",
      keywords: "settings.rows.snapDistance.keywords",
      control: {
        kind: "number",
        min: SNAP_DISTANCE_MIN,
        max: SNAP_DISTANCE_MAX,
        unit: "settings.rows.snapDistance.unit",
      },
    },
    {
      id: "timeline.follow",
      key: "timeline.follow",
      label: "settings.rows.follow.label",
      description: "settings.rows.follow.description",
      keywords: "settings.rows.follow.keywords",
      control: {
        kind: "segmented",
        options: choices(FOLLOW_MODES, {
          page: "settings.rows.follow.page",
          smooth: "settings.rows.follow.smooth",
          off: "settings.rows.follow.off",
        }),
      },
    },
    {
      // ADR-0037: the editor passes it as `main_magnetic` when it creates a project.
      id: "timeline.magnetic_main",
      key: "timeline.magnetic_main",
      label: "settings.rows.magneticMain.label",
      description: "settings.rows.magneticMain.description",
      keywords: "settings.rows.magneticMain.keywords",
      control: { kind: "switch" },
    },
    {
      id: "timeline.thumbnails",
      key: "timeline.thumbnails",
      label: "settings.rows.thumbnails.label",
      description: "settings.rows.thumbnails.description",
      keywords: "settings.rows.thumbnails.keywords",
      control: {
        kind: "segmented",
        options: choices(THUMBNAIL_MODES, {
          filmstrip: "settings.rows.thumbnails.filmstrip",
          first: "settings.rows.thumbnails.first",
          off: "settings.rows.thumbnails.off",
        }),
      },
    },
    {
      id: "timeline.waveforms",
      key: "timeline.waveforms",
      label: "settings.rows.waveforms.label",
      description: "settings.rows.waveforms.description",
      keywords: "settings.rows.waveforms.keywords",
      control: { kind: "switch" },
    },
    {
      id: "timeline.track_height",
      key: "timeline.track_height",
      label: "settings.rows.trackHeight.label",
      description: "settings.rows.trackHeight.description",
      keywords: "settings.rows.trackHeight.keywords",
      control: {
        kind: "segmented",
        options: choices(TRACK_HEIGHTS, {
          small: "settings.rows.trackHeight.small",
          medium: "settings.rows.trackHeight.medium",
          large: "settings.rows.trackHeight.large",
        }),
      },
    },
    {
      id: "timeline.wheel",
      key: "timeline.wheel",
      label: "settings.rows.wheel.label",
      description: "settings.rows.wheel.description",
      keywords: "settings.rows.wheel.keywords",
      control: {
        kind: "segmented",
        options: choices(WHEEL_MODES, {
          scroll: "settings.rows.wheel.scroll",
          zoom: "settings.rows.wheel.zoom",
        }),
      },
    },
    {
      id: "timeline.time_display",
      key: "timeline.time_display",
      label: "settings.rows.timeDisplay.label",
      description: "settings.rows.timeDisplay.description",
      keywords: "settings.rows.timeDisplay.keywords",
      control: {
        kind: "segmented",
        options: choices(TIME_DISPLAYS, {
          timecode: "settings.rows.timeDisplay.timecode",
          frames: "settings.rows.timeDisplay.frames",
          seconds: "settings.rows.timeDisplay.seconds",
        }),
      },
    },
    {
      id: "timeline.drop_frame",
      key: "timeline.drop_frame",
      label: "settings.rows.dropFrame.label",
      description: "settings.rows.dropFrame.description",
      keywords: "settings.rows.dropFrame.keywords",
      control: { kind: "switch" },
      disabledWhen: (values) => values["timeline.time_display"] !== "timecode",
    },
  ],
};

/** The Timeline page. */
export const TIMELINE_PAGE: SettingsPageDef = timelinePage;

/** Registers the Timeline page and the Editing rows (idempotent). */
export function registerTimelinePages(): void {
  for (const row of EDITING_ROWS) registerSettingRow("editing", row);
  registerSettingsPage(timelinePage);
}
registerTimelinePages();
