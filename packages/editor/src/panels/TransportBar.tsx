// SPDX-License-Identifier: GPL-3.0-or-later
import { IconButton, Toolbar, ToolbarSeparator, Tooltip } from "@cutweave/ui";
import { useTranslation } from "react-i18next";

/**
 * Transport placeholder under the preview (real playback arrives in WP-1M.04). Controls are
 * disabled until an engine is connected; names and order already match the final bar.
 */
export function TransportBar() {
  const { t } = useTranslation();
  return (
    <div className="cw-transport">
      <output className="cw-transport__time timecode-lg" aria-label={t("transport.timecode")}>
        {t("transport.timecodeValue")}
      </output>
      <Toolbar aria-label={t("transport.label")} className="cw-transport__bar">
        <Tooltip label={t("transport.toStart")}>
          <IconButton icon="skip-back" label={t("transport.toStart")} disabled />
        </Tooltip>
        <Tooltip label={t("transport.stepBack")}>
          <IconButton icon="step-back" label={t("transport.stepBack")} disabled />
        </Tooltip>
        <Tooltip label={t("transport.play")}>
          <IconButton icon="play" label={t("transport.play")} disabled />
        </Tooltip>
        <Tooltip label={t("transport.stepForward")}>
          <IconButton icon="step-forward" label={t("transport.stepForward")} disabled />
        </Tooltip>
        <Tooltip label={t("transport.toEnd")}>
          <IconButton icon="skip-forward" label={t("transport.toEnd")} disabled />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("transport.loop")}>
          <IconButton icon="repeat" label={t("transport.loop")} pressed={false} disabled />
        </Tooltip>
        <Tooltip label={t("transport.mute")}>
          <IconButton icon="volume-2" label={t("transport.mute")} disabled />
        </Tooltip>
        <Tooltip label={t("transport.fullscreen")}>
          <IconButton icon="maximize-2" label={t("transport.fullscreen")} disabled />
        </Tooltip>
      </Toolbar>
    </div>
  );
}
