// SPDX-License-Identifier: GPL-3.0-or-later
import { Icon, IconButton, Toolbar, ToolbarSeparator, Tooltip } from "@cutweave/ui";
import { useTranslation } from "react-i18next";

/** Lanes shown until the engine supplies tracks (WP-1M.05): one video, one audio. */
const PLACEHOLDER_TRACKS = [
  { kind: "video", n: 1 },
  { kind: "audio", n: 1 },
] as const;

/** Bottom dock panel: timeline tools and empty track lanes. The playhead sits at the start. */
export function TimelinePanel() {
  const { t } = useTranslation();
  return (
    <div className="cw-timeline">
      <Toolbar aria-label={t("timeline.toolbarLabel")} className="cw-timeline__tools">
        <Tooltip label={t("timeline.select")}>
          <IconButton icon="mouse-pointer-2" label={t("timeline.select")} pressed disabled />
        </Tooltip>
        <Tooltip label={t("timeline.razor")}>
          <IconButton icon="razor" label={t("timeline.razor")} pressed={false} disabled />
        </Tooltip>
        <Tooltip label={t("timeline.ripple")}>
          <IconButton icon="ripple" label={t("timeline.ripple")} pressed={false} disabled />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("timeline.snapping")}>
          <IconButton icon="magnet" label={t("timeline.snapping")} pressed disabled />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("timeline.zoomOut")}>
          <IconButton icon="zoom-out" label={t("timeline.zoomOut")} disabled />
        </Tooltip>
        <Tooltip label={t("timeline.zoomIn")}>
          <IconButton icon="zoom-in" label={t("timeline.zoomIn")} disabled />
        </Tooltip>
      </Toolbar>
      <div className="cw-timeline__body">
        <ul className="cw-timeline__lanes" aria-label={t("timeline.lanesLabel")}>
          {PLACEHOLDER_TRACKS.map(({ kind, n }) => (
            <li key={`${kind}-${n}`} className="cw-timeline__lane" data-kind={kind}>
              <span className="cw-timeline__head label">
                <Icon name={kind === "video" ? "film" : "audio-waveform"} size="md" />
                {kind === "video"
                  ? t("timeline.trackVideo", { n })
                  : t("timeline.trackAudio", { n })}
              </span>
            </li>
          ))}
        </ul>
        <div className="cw-timeline__playhead" aria-hidden="true" />
        <p className="cw-timeline__hint body">
          <span className="body-strong">{t("timeline.emptyTitle")}</span>
          {t("timeline.emptyBody")}
        </p>
      </div>
    </div>
  );
}
