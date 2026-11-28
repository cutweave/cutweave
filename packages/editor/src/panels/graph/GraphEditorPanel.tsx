// SPDX-License-Identifier: GPL-3.0-or-later
// The Graph Editor panel (WP-2C.07): the animated properties of the selected clips as curves of
// value over time. Keys are selected (a click, a box), dragged in time and value, added with a
// double-click and deleted; bezier handles shape a segment (free, aligned or mirrored across a
// key); the Time and Value fields take typed numbers. The canvas is `@cutweave/ui`'s
// `<GraphEditor>`, fed and listened to by `GraphController` without React in between: this
// component renders once, its toolbar and curve list only when what they show changes.
import {
  curveColorToken,
  GraphEditor,
  type GraphEditorHandle,
  type GraphOptions,
  IconButton,
  type IconName,
  SegmentedControl,
  type TangentMode,
  TextField,
  ToolbarSeparator,
  Tooltip,
} from "@cutweave/ui";
import {
  type KeyboardEvent,
  memo,
  type ReactElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import type { TKey } from "../../i18n/index.ts";
import { registerContextSurface } from "../../menus/host.tsx";
import type { KeyframeData } from "../../menus/surfaces/keyframes.ts";
import { useViewStore, viewStore } from "../../state/viewStore.ts";
import { translator } from "../../timeline/errors.ts";
import { transportController } from "../../transport/useTransport.ts";
import { EmptyState } from "../EmptyState.tsx";
import { defaultGraphDeps, GraphController } from "./graphController.ts";
import { graphShellStore, graphViewStore, type ShellReadout, toggleHidden } from "./graphStore.ts";

/** The canvas's options until the controller gives it the sequence's own (its first act). */
const STARTING_OPTIONS: GraphOptions = {
  second: 1,
  frame: 1,
  formatTime: String,
  formatValue: String,
};

type Kind = NonNullable<ShellReadout["kind"]>;

const KINDS: readonly { readonly kind: Kind; readonly icon: IconName; readonly label: TKey }[] = [
  { kind: "hold", icon: "ease-hold", label: "graph.toolbar.hold" },
  { kind: "linear", icon: "ease-linear", label: "graph.toolbar.linear" },
  { kind: "bezier", icon: "ease-bezier", label: "graph.toolbar.bezier" },
  { kind: "spring", icon: "ease-spring", label: "graph.toolbar.spring" },
];

function Tool({
  icon,
  label,
  pressed,
  disabled = false,
  onClick,
}: {
  readonly icon: IconName;
  readonly label: string;
  readonly pressed?: boolean;
  readonly disabled?: boolean;
  readonly onClick: () => void;
}): ReactElement {
  return (
    <Tooltip label={label}>
      <IconButton
        size="sm"
        icon={icon}
        label={label}
        disabled={disabled}
        onClick={onClick}
        {...(pressed !== undefined && { pressed })}
      />
    </Tooltip>
  );
}

/**
 * A typed number: it shows `value` until the person types, and commits on Enter or when the
 * focus leaves (Escape puts the shown value back). `commit` says whether the text was usable.
 */
function Entry({
  label,
  value,
  placeholder,
  disabled,
  epoch,
  commit,
}: {
  readonly label: string;
  readonly value: string;
  readonly placeholder: string;
  readonly disabled: boolean;
  /** Changes when a typed entry was refused: the draft is dropped. */
  readonly epoch: number;
  readonly commit: (text: string) => boolean;
}): ReactElement {
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  // A new value from the document, or a refusal, replaces what was typed.
  useEffect(() => {
    setDraft(null);
    setInvalid(false);
  }, [value, epoch]);
  const done = (): void => {
    if (draft === null || draft === value) {
      setDraft(null);
      return;
    }
    if (commit(draft)) setDraft(null);
    else setInvalid(true);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === "Enter") {
      e.preventDefault();
      done();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      setDraft(null);
      setInvalid(false);
    }
  };
  return (
    <span className="cw-graph-panel__entry">
      <span className="caption" aria-hidden="true">
        {label}
      </span>
      <TextField
        numeric
        aria-label={label}
        value={draft ?? value}
        placeholder={placeholder}
        disabled={disabled}
        invalid={invalid}
        onChange={(text) => {
          setDraft(text);
          setInvalid(false);
        }}
        onBlur={done}
        onKeyDown={onKeyDown}
      />
    </span>
  );
}

const GraphToolbar = memo(function GraphToolbar({
  controller,
}: {
  readonly controller: GraphController;
}): ReactElement {
  const { t } = useTranslation();
  const readout = useStore(graphShellStore, (s) => s.readout);
  const epoch = useStore(graphShellStore, (s) => s.epoch);
  const normalized = useStore(graphViewStore, (s) => s.normalized);
  const tangentMode = useStore(graphViewStore, (s) => s.tangentMode);
  const snapping = useViewStore((s) => s.snapping);
  const some = readout.count > 0 && readout.editable;
  const mixed = t("graph.toolbar.mixed");
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset draws a frame the tools have no room for
    <div className="cw-graph-panel__tools" role="group" aria-label={t("graph.toolbar.label")}>
      {/* biome-ignore lint/a11y/useSemanticElements: as above */}
      <div className="cw-graph-panel__group" role="group" aria-label={t("graph.toolbar.kind")}>
        {KINDS.map(({ kind, icon, label }) => (
          <Tool
            key={kind}
            icon={icon}
            label={t(label)}
            pressed={readout.kind === kind}
            disabled={!some || (readout.holdOnly && kind !== "hold")}
            onClick={() => void controller.setKind(kind)}
          />
        ))}
      </div>
      <Tool
        icon="sliders-horizontal"
        label={t("graph.toolbar.easing")}
        disabled={!some}
        onClick={() => controller.openEasing()}
      />
      <ToolbarSeparator />
      <SegmentedControl<TangentMode>
        aria-label={t("graph.toolbar.tangents")}
        value={tangentMode}
        onChange={(mode) => graphViewStore.setState({ tangentMode: mode })}
        options={[
          { value: "free", label: t("graph.toolbar.free") },
          { value: "aligned", label: t("graph.toolbar.aligned") },
          { value: "mirrored", label: t("graph.toolbar.mirrored") },
        ]}
      />
      <ToolbarSeparator />
      <Entry
        label={t("graph.toolbar.time")}
        value={readout.time === null ? "" : controller.formatTime(readout.time)}
        placeholder={readout.count > 1 ? mixed : ""}
        disabled={!some || readout.time === null}
        epoch={epoch}
        commit={(text) => controller.setTime(text)}
      />
      <Entry
        label={t("graph.toolbar.value")}
        value={readout.value === null ? "" : String(Number(readout.value.toPrecision(6)))}
        placeholder={readout.count > 0 && readout.value === null ? mixed : ""}
        disabled={!some || readout.holdOnly}
        epoch={epoch}
        commit={(text) => {
          const value = Number(text.trim().replace(",", "."));
          if (text.trim() === "" || !Number.isFinite(value)) return false;
          void controller.setValue(value);
          return true;
        }}
      />
      <Tool
        icon="trash-2"
        label={t("graph.toolbar.delete")}
        disabled={!some}
        onClick={() => void controller.action("delete")}
      />
      <ToolbarSeparator />
      <Tool
        icon="magnet"
        label={t("graph.toolbar.snapping")}
        pressed={snapping}
        onClick={() => viewStore.setState({ snapping: !snapping })}
      />
      <Tool
        icon="arrow-up-down"
        label={t("graph.toolbar.normalize")}
        pressed={normalized}
        onClick={() => graphViewStore.setState({ normalized: !normalized })}
      />
      <Tool
        icon="zoom-out"
        label={t("graph.toolbar.zoomOut")}
        onClick={() => controller.zoom(0.8)}
      />
      <Tool
        icon="zoom-in"
        label={t("graph.toolbar.zoomIn")}
        onClick={() => controller.zoom(1.25)}
      />
      <Tool icon="maximize-2" label={t("graph.toolbar.fit")} onClick={() => controller.fit()} />
    </div>
  );
});

/** The properties with curves: a colour per curve, and an eye that shows or hides them. */
const CurveList = memo(function CurveList(): ReactElement | null {
  const { t } = useTranslation();
  const props = useStore(graphShellStore, (s) => s.props);
  if (props.length === 0) return null;
  return (
    <ul className="cw-graph-panel__curves" aria-label={t("graph.curves.label")}>
      {props.map((p) => {
        const label = t(p.hidden ? "graph.curves.show" : "graph.curves.hide", { name: p.label });
        return (
          <li key={p.id} className="cw-graph-panel__curve" data-hidden={p.hidden || undefined}>
            <span className="cw-graph-panel__swatches" aria-hidden="true">
              {p.colors.map((color) => (
                <span
                  key={color}
                  className="cw-graph-panel__swatch"
                  style={{ background: `var(${curveColorToken(color)})` }}
                />
              ))}
            </span>
            <span className="cw-graph-panel__name label">{p.label}</span>
            <Tooltip label={label}>
              <IconButton
                size="sm"
                icon={p.hidden ? "eye-off" : "eye"}
                label={label}
                pressed={!p.hidden}
                onClick={() => toggleHidden(p.prop)}
              />
            </Tooltip>
          </li>
        );
      })}
    </ul>
  );
});

/** Shown over the empty graph while no selected clip has an animated property. */
const Empty = memo(function Empty(): ReactElement | null {
  const { t } = useTranslation();
  const none = useStore(graphShellStore, (s) => s.props.length === 0);
  if (!none) return null;
  return (
    <div className="cw-graph-panel__empty">
      <EmptyState icon="ease-bezier" title={t("graph.empty.title")} body={t("graph.empty.body")} />
    </div>
  );
});

export function GraphEditorPanel(): ReactElement {
  const { t } = useTranslation();
  const controller = useMemo(
    () =>
      new GraphController(
        defaultGraphDeps(translator(t), (at) => void transportController().seek(at)),
      ),
    [t],
  );
  const onView = useCallback(
    (handle: GraphEditorHandle | null) => {
      if (handle === null) controller.detach();
      else controller.attach(handle);
    },
    [controller],
  );
  useEffect(() => () => controller.detach(), [controller]);
  const labels = useMemo(() => ({ graph: t("graph.canvas"), help: t("graph.help") }), [t]);

  // A right-click on a key, or the menu key with one selected, opens the keyframe's menu (the
  // timeline's: Interpolation, Copy, Delete keyframe, Go to keyframe).
  const offMenu = useRef<(() => void) | null>(null);
  const surface = useCallback(
    (element: HTMLDivElement | null) => {
      offMenu.current?.();
      offMenu.current = null;
      if (element === null) return;
      offMenu.current = registerContextSurface(
        element,
        (query) => {
          // The menu key anchors at the selected key, a pointer where it is.
          const key =
            query.via === "keyboard"
              ? controller.selectedKey()
              : controller.keyAt(query.point.x, query.point.y);
          if (key === null) return null;
          const data: KeyframeData = { keyframeTime: key.at, prop: key.prop };
          return {
            surface: "timeline.keyframe",
            target: { ids: [key.item], data },
            ...(key.point !== null && { point: key.point }),
          };
        },
        { name: "graph.keyframe" },
      );
    },
    [controller],
  );

  return (
    <div className="cw-graph-panel">
      <GraphToolbar controller={controller} />
      <div className="cw-graph-panel__body">
        <CurveList />
        <div className="cw-graph-panel__canvas" ref={surface}>
          <GraphEditor options={STARTING_OPTIONS} onView={onView} labels={labels} />
          <Empty />
        </div>
      </div>
    </div>
  );
}
