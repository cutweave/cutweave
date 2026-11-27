// SPDX-License-Identifier: GPL-3.0-or-later
import { PLAYING_QUALITIES, type PlayingQualityChoice } from "@cutweave/preferences";
import type { PreviewQuality } from "@cutweave/protocol";
import {
  Icon,
  IconButton,
  Menu,
  type MenuEntry,
  Slider,
  StatusChip,
  Toolbar,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useEngineStore } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { useContextMenu } from "../menus/host.tsx";
import { preferences, usePref } from "../prefs/prefsStore.ts";
import { observeSize, windowOf } from "../shell/windows.ts";
import { useDocStore } from "../state/docStore.ts";
import { usePlaybackStore } from "../state/playbackStore.ts";
import { selectActiveSequenceId } from "../state/selectors.ts";
import { useViewStore } from "../state/viewStore.ts";
import { displayTime } from "../timeline/prefs.ts";
import { collapsedAt, fitLevel, MAX_LEVEL, recordNeed } from "../transport/barLayout.ts";
import { keymap } from "../transport/keymap.ts";
import { transportController } from "../transport/useTransport.ts";

/** Timecode base before a sequence exists (the readout shows zeros either way). */
const ZERO_RATE = { num: 30, den: 1 } as const;

const QUALITY_LABEL: Readonly<Record<PreviewQuality, TKey>> = {
  full: "transport.quality.full",
  half: "transport.quality.half",
  quarter: "transport.quality.quarter",
};

const PLAYING_QUALITY_LABEL: Readonly<Record<PlayingQualityChoice, TKey>> = {
  auto: "transport.quality.auto",
  full: "transport.quality.full",
  half: "transport.quality.half",
  quarter: "transport.quality.quarter",
};

/** The volumes the More menu offers while the slider is in it, in percent. */
const VOLUME_STEPS = [100, 75, 50, 25] as const;

const px = (value: string): number => Number.parseFloat(value) || 0;

/** The bar's width, and the width its three groups need side by side (CSS pixels). */
function measureBar(bar: HTMLElement): { available: number; need: number } {
  const style = windowOf(bar).getComputedStyle(bar);
  const groups = [...bar.children];
  // Each group is as wide as its content (`max-content` columns, see app.css).
  const content = groups.reduce((sum, el) => sum + el.getBoundingClientRect().width, 0);
  return {
    available: bar.getBoundingClientRect().width,
    need:
      content +
      px(style.columnGap) * Math.max(0, groups.length - 1) +
      px(style.paddingLeft) +
      px(style.paddingRight),
  };
}

/**
 * How many steps of `COLLAPSE_STEPS` are in the More menu for the width the bar has now (#225).
 * It follows the bar's own box (the panel can be docked narrow, floating or in its own window)
 * and the size of what is in it, so a density, a language or a time display that makes the
 * controls wider collapses them sooner.
 */
function useBarLevel(bar: HTMLElement | null): number {
  const [level, setLevel] = useState(0);
  const shown = useRef(0);
  const needs = useRef<(number | undefined)[]>([]);
  const fit = useCallback(() => {
    if (bar === null) return;
    const { available, need } = measureBar(bar);
    needs.current = recordNeed(needs.current, shown.current, need);
    const next = fitLevel(available, needs.current);
    if (next !== shown.current) setLevel(next);
  }, [bar]);
  // A new level is measured before it is painted, and one that does not fit is replaced at once.
  useLayoutEffect(() => {
    shown.current = level;
    fit();
  }, [level, fit]);
  // After that the bar and its three groups say when their size changes: the panel is resized,
  // or a text, the density or the fonts make a group wider or narrower.
  useEffect(() => {
    if (bar === null) return;
    const stops = [bar, ...bar.children].map((el) => observeSize(el, fit));
    return () => {
      for (const stop of stops) stop();
    };
  }, [bar, fit]);
  return level;
}

/**
 * `<TransportBar>` (WP-1M.04) under the preview: timecode (drop-frame at 29.97 and 59.94), go to
 * start, step back, play/pause, step forward, go to end, loop (the preference `playback.loop`,
 * WP-2H.10, which the runtime sends to the engine as `view.set_loop`), the quality of the frame
 * on screen (its menu chooses the quality while playing, `playback.quality_playing`), mute and
 * volume. Every button is a `view.*` engine command through the
 * transport; Space, J/K/L and the arrows reach the same actions through the keymap. Playback
 * controls are off without an engine, with an empty timeline, and for an engine that has no
 * transport.
 *
 * Proxies (WP-2F.02): a toggle turns the preview's use of proxies on or off (the preference
 * `playback.proxies`, which the runtime sends to the engine as `view.set_preview_quality`), and
 * a chip says what the frame on screen was decoded from, as the engine reports it: "Proxy" or
 * "Original".
 *
 * Width (#225): the bar keeps its controls apart at any width. Those that do not fit move into
 * the "More" menu at the end of the bar, least important first (`COLLAPSE_STEPS` in
 * `transport/barLayout.ts`), and come back when there is room. Play/pause and the timecode
 * always stay.
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
  const loop = usePref("playback.loop");
  const playingQuality = usePref("playback.quality_playing");
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
  // Settings > Timeline: timecode (drop-frame at 29.97 and 59.94 unless turned off), frames or
  // seconds.
  const timeDisplay = usePref("timeline.time_display");
  const dropFrame = usePref("timeline.drop_frame");
  const shown = { "timeline.time_display": timeDisplay, "timeline.drop_frame": dropFrame };
  const playLabel = playing ? t("transport.pause") : t("transport.play");
  const muteLabel = muted ? t("transport.unmute") : t("transport.mute");
  const sourceLabel = showsProxy ? t("transport.source.proxy") : t("transport.source.original");
  const qualityLabel = t("transport.qualityValue", { quality: t(QUALITY_LABEL[quality]) });
  // Right-click or the Menu key: Copy timecode, the volume's Mute and Reset, the quality choices.
  const timeRef = useContextMenu<HTMLOutputElement>("transport.time", () => ({}));
  const volumeRef = useContextMenu<HTMLDivElement>("transport.volume", () => ({}));
  const qualityRef = useContextMenu<HTMLOutputElement>("transport.quality", () => ({}));

  const [bar, setBar] = useState<HTMLDivElement | null>(null);
  const level = useBarLevel(bar);
  const inMore = collapsedAt(level);

  // The More menu: what is not in the bar, in the bar's order, each row doing what its control
  // does.
  const navigate: MenuEntry[] = [];
  if (inMore.has("toStart")) {
    navigate.push({
      id: "to-start",
      label: t("transport.toStart"),
      icon: "skip-back",
      disabled: off,
      onSelect: () => void transport.toStart(),
    });
  }
  if (inMore.has("stepBack")) {
    navigate.push({
      id: "step-back",
      label: t("transport.stepBack"),
      icon: "step-back",
      shortcut: keymap.keysFor("transport:stepBack")[0],
      disabled: off,
      keepOpen: true,
      onSelect: () => void transport.step(-1),
    });
  }
  if (inMore.has("stepForward")) {
    navigate.push({
      id: "step-forward",
      label: t("transport.stepForward"),
      icon: "step-forward",
      shortcut: keymap.keysFor("transport:stepForward")[0],
      disabled: off,
      keepOpen: true,
      onSelect: () => void transport.step(1),
    });
  }
  if (inMore.has("toEnd")) {
    navigate.push({
      id: "to-end",
      label: t("transport.toEnd"),
      icon: "skip-forward",
      disabled: off,
      onSelect: () => void transport.toEnd(),
    });
  }
  const sound: MenuEntry[] = [];
  if (inMore.has("loop")) {
    sound.push({
      id: "loop",
      label: t("transport.loop"),
      checked: loop,
      onSelect: () => void preferences.set("playback.loop", !loop),
    });
  }
  if (inMore.has("mute")) {
    sound.push({
      id: "mute",
      label: t("transport.mute"),
      checked: muted,
      disabled: !connected,
      onSelect: () => transport.toggleMute(),
    });
  }
  if (inMore.has("volume")) {
    sound.push({
      kind: "submenu",
      id: "volume",
      label: t("transport.volume"),
      disabled: !connected,
      items: VOLUME_STEPS.map((value) => ({
        id: `volume-${value}`,
        label: t("transport.volumeValue", { value }),
        checked: volume === value,
        choice: "radio" as const,
        onSelect: () => transport.setVolume(value / 100),
      })),
    });
  }
  const picture: MenuEntry[] = [];
  if (inMore.has("quality")) {
    picture.push(
      { kind: "label", id: "quality-shown", label: qualityLabel },
      {
        kind: "submenu",
        id: "quality",
        label: t("transport.qualityPlaying"),
        items: PLAYING_QUALITIES.map((choice) => ({
          id: `quality-${choice}`,
          label: t(PLAYING_QUALITY_LABEL[choice]),
          checked: playingQuality === choice,
          choice: "radio" as const,
          onSelect: () => void preferences.set("playback.quality_playing", choice),
        })),
      },
    );
  }
  if (inMore.has("source")) {
    picture.push({
      kind: "label",
      id: "source-shown",
      label: t("transport.sourceValue", { source: sourceLabel }),
    });
  }
  if (inMore.has("proxies")) {
    picture.push({
      id: "proxies",
      label: t("transport.useProxies"),
      checked: useProxies,
      onSelect: () => void preferences.set("playback.proxies", !useProxies),
    });
  }
  if (inMore.has("fullscreen")) {
    picture.push({ id: "fullscreen", label: t("transport.fullscreen"), disabled: true });
  }
  const moreItems = [navigate, sound, picture]
    .filter((group) => group.length > 0)
    .flatMap((group, index): MenuEntry[] =>
      index === 0 ? group : [{ kind: "separator", id: `sep-${index}` }, ...group],
    );

  return (
    <div ref={setBar} className="cw-transport" data-fit={level === MAX_LEVEL ? "tight" : undefined}>
      <output
        ref={timeRef}
        className="cw-transport__time timecode-lg"
        aria-label={t("transport.timecode")}
        data-source={source !== null}
      >
        {source === null
          ? displayTime(playhead, fps ?? ZERO_RATE, shown)
          : displayTime(sourcePlayhead, source.fps, shown)}
      </output>
      <Toolbar aria-label={t("transport.label")} className="cw-transport__bar">
        {inMore.has("toStart") ? null : (
          <Tooltip label={t("transport.toStart")}>
            <IconButton
              icon="skip-back"
              label={t("transport.toStart")}
              disabled={off}
              onClick={() => void transport.toStart()}
            />
          </Tooltip>
        )}
        {inMore.has("stepBack") ? null : (
          <Tooltip label={t("transport.stepBack")} keys={keymap.keysFor("transport:stepBack")}>
            <IconButton
              icon="step-back"
              label={t("transport.stepBack")}
              disabled={off}
              onClick={() => void transport.step(-1)}
            />
          </Tooltip>
        )}
        <Tooltip label={playLabel} keys={keymap.keysFor("transport:togglePlay")}>
          <IconButton
            icon={playing ? "pause" : "play"}
            label={playLabel}
            disabled={off}
            onClick={() => void transport.togglePlay()}
          />
        </Tooltip>
        {inMore.has("stepForward") ? null : (
          <Tooltip
            label={t("transport.stepForward")}
            keys={keymap.keysFor("transport:stepForward")}
          >
            <IconButton
              icon="step-forward"
              label={t("transport.stepForward")}
              disabled={off}
              onClick={() => void transport.step(1)}
            />
          </Tooltip>
        )}
        {inMore.has("toEnd") ? null : (
          <Tooltip label={t("transport.toEnd")}>
            <IconButton
              icon="skip-forward"
              label={t("transport.toEnd")}
              disabled={off}
              onClick={() => void transport.toEnd()}
            />
          </Tooltip>
        )}
        {inMore.has("loop") ? null : (
          <>
            <ToolbarSeparator />
            <Tooltip label={t("transport.loop")}>
              <IconButton
                icon="repeat"
                label={t("transport.loop")}
                pressed={loop}
                onClick={() => void preferences.set("playback.loop", !loop)}
              />
            </Tooltip>
          </>
        )}
      </Toolbar>
      <div ref={volumeRef} className="cw-transport__side">
        {inMore.has("quality") ? null : (
          <output
            ref={qualityRef}
            className="cw-transport__quality label"
            aria-label={qualityLabel}
          >
            <Icon name="gauge" size="sm" />
            {t(QUALITY_LABEL[quality])}
          </output>
        )}
        {inMore.has("source") ? null : (
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
        )}
        {inMore.has("proxies") ? null : (
          <Tooltip label={t("transport.useProxies")}>
            <IconButton
              icon="layers"
              size="sm"
              label={t("transport.useProxies")}
              pressed={useProxies}
              onClick={() => void preferences.set("playback.proxies", !useProxies)}
            />
          </Tooltip>
        )}
        {inMore.has("mute") ? null : (
          <Tooltip label={muteLabel}>
            <IconButton
              icon={muted ? "volume-x" : "volume-2"}
              size="sm"
              label={muteLabel}
              pressed={muted}
              disabled={!connected}
              onClick={() => transport.toggleMute()}
            />
          </Tooltip>
        )}
        {inMore.has("volume") ? null : (
          <Slider
            className="cw-transport__volume"
            label={t("transport.volume")}
            labelHidden
            value={volume}
            min={0}
            max={100}
            step={1}
            disabled={!connected}
            onChange={(v) => transport.setVolume(v / 100)}
            format={(v) => t("transport.volumeValue", { value: v })}
          />
        )}
        {inMore.has("fullscreen") ? null : (
          <Tooltip label={t("transport.fullscreen")}>
            <IconButton icon="maximize-2" size="sm" label={t("transport.fullscreen")} disabled />
          </Tooltip>
        )}
        {moreItems.length === 0 ? null : (
          <Menu mode="dropdown" items={moreItems} aria-label={t("transport.moreMenu")}>
            <Tooltip label={t("transport.more")}>
              <IconButton icon="ellipsis" size="sm" label={t("transport.more")} />
            </Tooltip>
          </Menu>
        )}
      </div>
    </div>
  );
}
