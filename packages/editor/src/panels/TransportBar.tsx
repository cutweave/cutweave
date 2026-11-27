// SPDX-License-Identifier: GPL-3.0-or-later
import type { PreviewQuality } from "@cutweave/protocol";
import { formatTimecode } from "@cutweave/timeline";
import {
  Icon,
  IconButton,
  Slider,
  StatusChip,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { useContextMenu } from "../menus/host.tsx";
import { preferences, usePref } from "../prefs/prefsStore.ts";
import { useDocStore } from "../state/docStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import { keymap } from "../transport/keymap.ts";
import { transportController } from "../transport/useTransport.ts";

/** Timecode base before a sequence exists (the readout shows zeros either way). */
const ZERO_RATE = { num: 30, den: 1 } as const;

const QUALITY_LABEL: Readonly<Record<PreviewQuality, TKey>> = {
  full: "transport.quality.full",
  half: "transport.quality.half",
  quarter: "transport.quality.quarter",
};

/**
 * `<TransportBar>` (WP-1M.04) under the preview: timecode (drop-frame at 29.97 and 59.94), go to
 * start, step back, play/pause, step forward, go to end, loop (off in v1), the quality of the
 * frame on screen, mute and volume. Every button is a `view.*` engine command through the
 * transport; Space, J/K/L and the arrows reach the same actions through the keymap. Playback
 * controls are off without an engine, with an empty timeline, and for an engine that has no
 * transport.
 *
 * Proxies (WP-2F.02): a toggle turns the preview's use of proxies on or off (the preference
 * `playback.proxies`, which the runtime sends to the engine as `view.set_preview_quality`), and
 * a chip says what the frame on screen was decoded from, as the engine reports it: "Proxy" or
 * "Original".
 */
export function TransportBar() {
  const { t } = useTranslation();
  const transport = transportController();
  const connected = useEngineStore((s) => s.connection === "ready");
  const support = usePlaybackStore((s) => s.transport);
  const playing = usePlaybackStore((s) => s.playing);
  const quality = usePlaybackStore((s) => s.quality);
  const showsProxy = usePlaybackStore((s) => s.proxy);
  const useProxies = usePref("playback.proxies");
  const source = usePlaybackStore((s) => s.source);
  const sourcePlayhead = usePlaybackStore((s) => s.sourcePlayhead);
  const playhead = useViewStore((s) => s.playhead);
  const gain = useViewStore((s) => s.masterGain);
  const muted = useViewStore((s) => s.muted);
  const project = useDocStore((s) => s.project);
  const { fps, hasContent } = useMemo(() => {
    const id = selectActiveSequenceId(project);
    const sequence = id === null ? undefined : project?.sequences[id];
    return {
      fps: sequence?.format.frame_rate,
      hasContent: Object.values(sequence?.tracks ?? {}).some(
        (track) => Object.keys(track.items).length > 0,
      ),
    };
  }, [project]);
  // A source plays whatever the timeline holds, and the buttons then act on it.
  const live = connected && (hasContent || source !== null) && support !== "unsupported";
  const off = !live;
  const volume = Math.round(gain * 100);
  const playLabel = playing ? t("transport.pause") : t("transport.play");
  const sourceLabel = showsProxy ? t("transport.source.proxy") : t("transport.source.original");
  // Right-click or the Menu key: Copy timecode, the volume's Mute and Reset, the quality choices.
  const timeRef = useContextMenu<HTMLOutputElement>("transport.time", () => ({}));
  const volumeRef = useContextMenu<HTMLDivElement>("transport.volume", () => ({}));
  const qualityRef = useContextMenu<HTMLOutputElement>("transport.quality", () => ({}));

  return (
    <div className="cw-transport">
      <output
        ref={timeRef}
        className="cw-transport__time timecode-lg"
        aria-label={t("transport.timecode")}
        data-source={source !== null}
      >
        {source === null
          ? formatTimecode(playhead, fps ?? ZERO_RATE, true)
          : formatTimecode(sourcePlayhead, source.fps, true)}
      </output>
      <Toolbar aria-label={t("transport.label")} className="cw-transport__bar">
        <Tooltip label={t("transport.toStart")}>
          <IconButton
            icon="skip-back"
            label={t("transport.toStart")}
            disabled={off}
            onClick={() => void transport.toStart()}
          />
        </Tooltip>
        <Tooltip label={t("transport.stepBack")} keys={keymap.keysFor("transport:stepBack")}>
          <IconButton
            icon="step-back"
            label={t("transport.stepBack")}
            disabled={off}
            onClick={() => void transport.step(-1)}
          />
        </Tooltip>
        <Tooltip label={playLabel} keys={keymap.keysFor("transport:togglePlay")}>
          <IconButton
            icon={playing ? "pause" : "play"}
            label={playLabel}
            disabled={off}
            onClick={() => void transport.togglePlay()}
          />
        </Tooltip>
        <Tooltip label={t("transport.stepForward")} keys={keymap.keysFor("transport:stepForward")}>
          <IconButton
            icon="step-forward"
            label={t("transport.stepForward")}
            disabled={off}
            onClick={() => void transport.step(1)}
          />
        </Tooltip>
        <Tooltip label={t("transport.toEnd")}>
          <IconButton
            icon="skip-forward"
            label={t("transport.toEnd")}
            disabled={off}
            onClick={() => void transport.toEnd()}
          />
        </Tooltip>
        <ToolbarSeparator />
        <Tooltip label={t("transport.loop")}>
          <IconButton icon="repeat" label={t("transport.loop")} pressed={false} disabled />
        </Tooltip>
      </Toolbar>
      <div ref={volumeRef} className="cw-transport__side">
        <output
          ref={qualityRef}
          className="cw-transport__quality label"
          aria-label={t("transport.qualityValue", { quality: t(QUALITY_LABEL[quality]) })}
        >
          <Icon name="gauge" size="sm" />
          {t(QUALITY_LABEL[quality])}
        </output>
        <Tooltip label={t("transport.sourceHint")}>
          <output
            className="cw-transport__source"
            aria-label={t("transport.sourceValue", { source: sourceLabel })}
            data-source={showsProxy ? "proxy" : "original"}
          >
            <StatusChip tone="neutral" icon={showsProxy ? "layers" : "film"}>
              {sourceLabel}
            </StatusChip>
          </output>
        </Tooltip>
        <Tooltip label={t("transport.useProxies")}>
          <IconButton
            icon="layers"
            size="sm"
            label={t("transport.useProxies")}
            pressed={useProxies}
            onClick={() => void preferences.set("playback.proxies", !useProxies)}
          />
        </Tooltip>
        <Tooltip label={muted ? t("transport.unmute") : t("transport.mute")}>
          <IconButton
            icon={muted ? "volume-x" : "volume-2"}
            size="sm"
            label={muted ? t("transport.unmute") : t("transport.mute")}
            pressed={muted}
            disabled={!connected}
            onClick={() => transport.toggleMute()}
          />
        </Tooltip>
        <Slider
          className="cw-transport__volume"
          label={t("transport.volume")}
          value={volume}
          min={0}
          max={100}
          step={1}
          disabled={!connected}
          onChange={(v) => transport.setVolume(v / 100)}
          format={(v) => t("transport.volumeValue", { value: v })}
        />
        <Tooltip label={t("transport.fullscreen")}>
          <IconButton icon="maximize-2" size="sm" label={t("transport.fullscreen")} disabled />
        </Tooltip>
      </div>
    </div>
  );
}
