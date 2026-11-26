// SPDX-License-Identifier: GPL-3.0-or-later
import {
  createContext,
  memo,
  type ReactNode,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { Button } from "../components/Button.tsx";
import { cx } from "../components/cx.ts";
import { IconButton } from "../components/IconButton.tsx";
import { InspectorSection } from "../components/InspectorSection.tsx";
import { KeyframeGroup } from "../components/KeyframeToggle.tsx";
import { compileSchema, defaultOf, type FieldNode } from "./compile.ts";
import {
  AngleControl,
  AssetControl,
  ColorControl,
  type ControlProps,
  CurveControl,
  EnumSelectControl,
  FontControl,
  NumberControl,
  SegmentedControlField,
  SliderControl,
  TextAreaControl,
  TextControl,
  ToggleControl,
  UnsupportedControl,
  Vec2Control,
} from "./controls.tsx";
import { GestureBus } from "./gesture.ts";
import {
  type AssetOption,
  DEFAULT_STRINGS,
  type FieldKeyframe,
  type FormRenderers,
  type FormStrings,
  type JsonSchema,
  type OnChange,
  type UiHints,
} from "./types.ts";
import { deepEqual, getPath } from "./values.ts";
import { evalVisible } from "./visibleWhen.ts";

export interface SchemaFormProps {
  /** A JSON Schema object (a command's params) with optional `x-ui` hints. */
  schema: JsonSchema;
  /** The current values. The form is controlled: it shows `value` and reports edits. */
  value: unknown;
  /** Extra hints by dot path; they override the schema's own `x-ui`. */
  hints?: Readonly<Record<string, UiHints>> | undefined;
  /**
   * Every edit: `(path, value, {gesture})`. A drag is `start`, throttled `update`s (at most 60
   * per second) and exactly one `end`; a toggle, typed value or reset has no `gesture`.
   */
  onChange: OnChange;
  /** Dot paths where a multi-selection disagrees; those fields show "Mixed". */
  mixed?: readonly string[] | ReadonlySet<string> | undefined;
  /** The diamond per keyframeable path. Without this prop no field shows one. */
  keyframes?: Readonly<Record<string, FieldKeyframe>> | undefined;
  /** Defaults by path, for values the schema does not carry (a property's spec default). */
  defaults?: Readonly<Record<string, unknown>> | undefined;
  /** A row's reset. Without it, reset sends the default through `onChange`. */
  onReset?: ((path: string) => void) | undefined;
  /** A section's reset (the section's modified paths). Without it, sections show none. */
  onResetGroup?: ((group: string, paths: readonly string[]) => void) | undefined;
  /** The bin, for `asset` fields (filtered by `x-ui.kind`). */
  assets?: readonly AssetOption[] | undefined;
  /** Slots for editors that live outside this package (the `curve` editor). */
  renderers?: FormRenderers | undefined;
  disabled?: boolean | undefined;
  strings?: Partial<FormStrings> | undefined;
  /** Accessible name of the form. */
  "aria-label"?: string | undefined;
  className?: string | undefined;
}

interface FormContext {
  bus: GestureBus;
  strings: FormStrings;
  assets: readonly AssetOption[] | undefined;
  renderers: FormRenderers;
  disabled: boolean;
  isMixed: (path: string) => boolean;
  defaults: Readonly<Record<string, unknown>> | undefined;
  /** Stable wrappers: they read the latest prop. */
  onReset: (path: string) => void;
  hasOnReset: boolean;
  onResetGroup: ((group: string, paths: readonly string[]) => void) | undefined;
  idBase: string;
  expanded: Readonly<Record<string, boolean>>;
  setExpanded: (key: string, open: boolean) => void;
}

/** What changes with every edit; only the lists read it, so a row re-renders when its value does. */
interface LiveContext {
  rootValue: unknown;
  drafts: Readonly<Record<string, unknown>>;
  keyframes: Readonly<Record<string, FieldKeyframe>> | undefined;
}

const Ctx = createContext<FormContext | null>(null);
const Live = createContext<LiveContext | null>(null);

function useLive(): LiveContext {
  const live = useContext(Live);
  if (!live) throw new Error("SchemaForm field outside a SchemaForm");
  return live;
}

function useForm(): FormContext {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("SchemaForm field outside a SchemaForm");
  return ctx;
}

/** Schema in, accessible form out. See SCHEMA_HINTS.md for the `x-ui` format. */
export function SchemaForm({
  schema,
  value,
  hints,
  onChange,
  mixed,
  keyframes,
  defaults,
  onReset,
  onResetGroup,
  assets,
  renderers,
  disabled = false,
  strings,
  "aria-label": ariaLabel,
  className,
}: SchemaFormProps) {
  const nodes = useMemo(() => compileSchema(schema, { hints }), [schema, hints]);
  const [drafts, setDrafts] = useState<Readonly<Record<string, unknown>>>({});
  const [expanded, setExpandedState] = useState<Readonly<Record<string, boolean>>>({});
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const bus = useMemo(
    () =>
      new GestureBus(
        () => onChangeRef.current,
        (path, next, active) =>
          setDrafts((d) => {
            if (active) return { ...d, [path]: next };
            if (!(path in d)) return d;
            const { [path]: _gone, ...rest } = d;
            return rest;
          }),
      ),
    [],
  );
  useEffect(() => () => bus.dispose(), [bus]);

  const mixedKey = mixed === undefined ? "" : [...mixed].sort().join("\u0000");
  const mixedSet = useMemo(() => new Set(mixed ?? []), [mixedKey]);
  const merged = useMemo(() => ({ ...DEFAULT_STRINGS, ...strings }), [strings]);
  const idBase = useId();
  const onResetRef = useRef(onReset);
  onResetRef.current = onReset;
  const hasOnReset = onReset !== undefined;
  const onResetGroupRef = useRef(onResetGroup);
  onResetGroupRef.current = onResetGroup;
  const hasOnResetGroup = onResetGroup !== undefined;
  const ctx = useMemo<FormContext>(
    () => ({
      bus,
      strings: merged,
      assets,
      renderers: renderers ?? {},
      disabled,
      isMixed: (path) => {
        if (mixedSet.size === 0) return false;
        for (
          let at = path;
          at !== "";
          at = at.includes(".") ? at.slice(0, at.lastIndexOf(".")) : ""
        ) {
          if (mixedSet.has(at)) return true;
        }
        return mixedSet.has("");
      },
      defaults,
      onReset: (path) => onResetRef.current?.(path),
      hasOnReset,
      onResetGroup: hasOnResetGroup ? (g, paths) => onResetGroupRef.current?.(g, paths) : undefined,
      idBase,
      expanded,
      setExpanded: (key, open) => setExpandedState((e) => ({ ...e, [key]: open })),
    }),
    [
      bus,
      merged,
      assets,
      renderers,
      disabled,
      mixedSet,
      defaults,
      hasOnReset,
      hasOnResetGroup,
      idBase,
      expanded,
    ],
  );
  const live = useMemo<LiveContext>(
    () => ({ rootValue: value, drafts, keyframes }),
    [value, drafts, keyframes],
  );

  return (
    <Ctx.Provider value={ctx}>
      <Live.Provider value={live}>
        {/* biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout */}
        <div role="group" aria-label={ariaLabel} className={cx("cw-sf", className)}>
          <FieldList nodes={nodes} container={value} nested={false} scope="" />
        </div>
      </Live.Provider>
    </Ctx.Provider>
  );
}

// ---------- Lists, groups, sections ----------

function effective(live: LiveContext, node: FieldNode, committed: unknown): unknown {
  return node.path in live.drafts ? live.drafts[node.path] : committed;
}

function isModified(ctx: FormContext, node: FieldNode, value: unknown): boolean {
  const def = defaultOf(node, ctx.defaults);
  if (!def.has) return false;
  if (ctx.isMixed(node.path)) return true;
  return value !== undefined && !deepEqual(value, def.value);
}

function modifiedPaths(
  ctx: FormContext,
  live: LiveContext,
  nodes: readonly FieldNode[],
  container: unknown,
): string[] {
  const out: string[] = [];
  for (const node of nodes) {
    const v = effective(live, node, getPath(container, node.key));
    if (node.widget === "object")
      out.push(...modifiedPaths(ctx, live, node.children, getPath(container, node.key)));
    else if (isModified(ctx, node, v)) out.push(node.path);
  }
  return out;
}

function FieldList({
  nodes,
  container,
  nested,
  scope,
}: {
  nodes: readonly FieldNode[];
  container: unknown;
  nested: boolean;
  scope: string;
}) {
  const ctx = useForm();
  const live = useLive();
  const visible = nodes.filter(
    (n) =>
      !n.visibleWhen ||
      evalVisible(n.visibleWhen, (p) => {
        const local = getPath(container, p);
        return local !== undefined ? local : getPath(live.rootValue, p);
      }),
  );
  const row = (node: FieldNode) => (
    <FieldView
      key={node.path}
      node={node}
      value={effective(live, node, getPath(container, node.key))}
      mixed={ctx.isMixed(node.path)}
      keyframe={live.keyframes?.[node.path]}
      hasKeyframes={live.keyframes !== undefined}
    />
  );
  if (nested) return <>{visible.map(row)}</>;

  const plain = visible.filter((n) => !n.advanced && n.group === undefined);
  const groups: string[] = [];
  for (const n of visible) {
    if (!n.advanced && n.group !== undefined && !groups.includes(n.group)) groups.push(n.group);
  }
  const advanced = visible.filter((n) => n.advanced);
  const section = (
    key: string,
    label: string,
    members: readonly FieldNode[],
    open: boolean,
    group?: string,
  ) => {
    const paths = modifiedPaths(ctx, live, members, container);
    const resetGroup = ctx.onResetGroup;
    return (
      <InspectorSection
        key={key}
        label={label}
        expanded={ctx.expanded[`${scope}#${key}`] ?? open}
        onExpandedChange={(next) => ctx.setExpanded(`${scope}#${key}`, next)}
        modified={paths.length > 0}
        onReset={resetGroup ? () => resetGroup(group ?? key, paths) : undefined}
      >
        {members.map(row)}
      </InspectorSection>
    );
  };
  return (
    <>
      {plain.length > 0 ? <div className="cw-sf-plain">{plain.map(row)}</div> : null}
      {groups.map((g) =>
        section(
          g,
          g,
          visible.filter((n) => !n.advanced && n.group === g),
          true,
          g,
        ),
      )}
      {advanced.length > 0 ? section("advanced", ctx.strings.advanced, advanced, false) : null}
    </>
  );
}

// ---------- One field ----------

interface FieldViewProps {
  node: FieldNode;
  value: unknown;
  mixed: boolean;
  keyframe: FieldKeyframe | undefined;
  hasKeyframes: boolean;
}

const STACKED = new Set(["object", "array", "tuple", "textarea"]);
/** Controls a `<label htmlFor>` can name (a native input, select or button). */
const LABELLED_BY_FOR = new Set([
  "text",
  "textarea",
  "select",
  "asset",
  "color",
  "toggle",
  "font",
  "curve",
]);

const FieldView = memo(function FieldView({
  node,
  value,
  mixed,
  keyframe,
  hasKeyframes,
}: FieldViewProps) {
  const ctx = useForm();
  const id = `${ctx.idBase}-${node.path}-c`;
  const labelId = `${ctx.idBase}-${node.path}-l`;
  const { bus, strings } = ctx;
  const disabled = ctx.disabled;

  if (node.widget === "object") {
    return (
      // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
      <div
        role="group"
        aria-labelledby={labelId}
        className="cw-sf-object"
        data-path={node.path}
        data-widget="object"
      >
        <span id={labelId} className="cw-sf-object__title overline">
          {node.label}
        </span>
        <FieldList nodes={node.children} container={value} nested scope={node.path} />
      </div>
    );
  }

  const control: ControlProps = {
    node,
    label: node.label,
    id,
    labelId,
    value,
    mixed,
    disabled,
    strings,
    live: (next) => bus.change(node.path, next, value),
    commit: (next) => bus.commit(node.path, next),
    cancel: () => bus.cancel(node.path),
  };

  let body: ReactNode;
  switch (node.widget) {
    case "number":
      body = <NumberControl {...control} />;
      break;
    case "slider":
      body = <SliderControl {...control} />;
      break;
    case "angle":
      body = <AngleControl {...control} />;
      break;
    case "toggle":
      body = <ToggleControl {...control} />;
      break;
    case "segmented":
      body = <SegmentedControlField {...control} />;
      break;
    case "select":
      body = <EnumSelectControl {...control} />;
      break;
    case "curve":
      body = <CurveControl {...control} renderers={ctx.renderers} />;
      break;
    case "color":
      body = <ColorControl {...control} />;
      break;
    case "font":
      body = <FontControl {...control} />;
      break;
    case "asset":
      body = <AssetControl {...control} assets={ctx.assets} />;
      break;
    case "text":
      body = <TextControl {...control} />;
      break;
    case "textarea":
      body = <TextAreaControl {...control} />;
      break;
    case "vec2":
      body = <Vec2Control {...control} mixedAxis={(axis) => ctx.isMixed(`${node.path}.${axis}`)} />;
      break;
    case "tuple":
      body = <TupleControl node={node} value={value} mixed={mixed} />;
      break;
    case "array":
      body = <ArrayControl node={node} value={value} />;
      break;
    default:
      body = <UnsupportedControl value={value} strings={strings} />;
  }

  const def = defaultOf(node, ctx.defaults);
  // A mixed field offers reset too (to unify the selection at the default); an unset one does not.
  const modified = def.has && (mixed || (value !== undefined && !deepEqual(value, def.value)));
  const reset = () => {
    if (ctx.hasOnReset) ctx.onReset(node.path);
    else bus.commit(node.path, def.value);
  };
  const showKeyframe = keyframe !== undefined || (hasKeyframes && node.keyframeable);
  const labelProps = { id: labelId, className: "cw-row__label label", title: node.description };
  // A pair needs the row's width for its two fields; with the keyframe group and reset beside it
  // in a narrow dock the control would run under them, so the label gets its own line.
  const stacked = STACKED.has(node.widget) || (node.widget === "vec2" && showKeyframe);
  return (
    <div
      className="cw-row cw-sf-row"
      data-path={node.path}
      data-widget={node.widget}
      data-layout={stacked ? "stack" : undefined}
      data-modified={modified ? "true" : undefined}
      data-mixed={mixed ? "true" : undefined}
      data-animated={keyframe && keyframe.state !== "off" ? keyframe.state : undefined}
    >
      {LABELLED_BY_FOR.has(node.widget) && node.widget !== "curve" ? (
        <label htmlFor={id} {...labelProps}>
          {node.label}
        </label>
      ) : (
        <span {...labelProps}>{node.label}</span>
      )}
      {body}
      {showKeyframe ? (
        <KeyframeGroup
          property={node.label}
          state={keyframe?.state ?? "off"}
          onToggle={keyframe?.onToggle ?? (() => {})}
          onPrev={keyframe?.onPrev}
          onNext={keyframe?.onNext}
          disabled={keyframe === undefined || keyframe.disabled === true || disabled}
        />
      ) : null}
      {modified ? (
        <IconButton
          size="sm"
          icon="undo-2"
          label={strings.reset(node.label)}
          className="cw-row__reset"
          disabled={disabled}
          onClick={reset}
        />
      ) : null}
    </div>
  );
});

// ---------- Tuple and array ----------

function TupleControl({ node, value, mixed }: { node: FieldNode; value: unknown; mixed: boolean }) {
  const items = Array.isArray(value) ? value : [];
  const cells = useMemo(
    () =>
      Array.from({ length: node.tupleLength }, (_, i) =>
        node.itemAt?.(i, `${node.label} ${i + 1}`),
      ),
    [node],
  );
  const ctx = useForm();
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
    <span role="group" aria-label={node.label} className="cw-sf-tuple">
      {cells.map((cell, i) =>
        cell ? (
          <FieldCell
            key={i}
            cell={cell}
            value={items[i]}
            mixed={mixed || ctx.isMixed(cell.path)}
            whole={items}
            parent={node}
            index={i}
          />
        ) : null,
      )}
    </span>
  );
}

/** One number of a tuple: edits report the whole array, so one command carries every axis. */
function FieldCell({
  cell,
  value,
  mixed,
  whole,
  parent,
  index,
}: {
  cell: FieldNode;
  value: unknown;
  mixed: boolean;
  whole: readonly unknown[];
  parent: FieldNode;
  index: number;
}) {
  const ctx = useForm();
  const next = (n: unknown) => {
    const copy: unknown[] = Array.from({ length: parent.tupleLength }, (_, i) => whole[i] ?? 0);
    copy[index] = n;
    return copy;
  };
  const props: ControlProps = {
    node: cell,
    label: cell.label,
    id: "",
    labelId: "",
    value,
    mixed,
    disabled: ctx.disabled,
    strings: ctx.strings,
    live: (n) => ctx.bus.change(parent.path, next(n), whole),
    commit: (n) => ctx.bus.commit(parent.path, next(n)),
    cancel: () => ctx.bus.cancel(parent.path),
  };
  return <NumberControl {...props} />;
}

function ArrayControl({ node, value }: { node: FieldNode; value: unknown }) {
  const ctx = useForm();
  const live = useLive();
  const items = Array.isArray(value) ? value : [];
  const { strings } = ctx;
  const item = (index: number) => node.itemAt?.(index, strings.item(node.label, index));
  const nodes = useMemo(
    () => items.map((_, i) => node.itemAt?.(i, strings.item(node.label, i))),
    [node, items.length, strings],
  );
  const canAdd = node.maxItems === undefined || items.length < node.maxItems;
  const canRemove = node.minItems === undefined || items.length > node.minItems;
  const seed = item(items.length)?.seed();
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset would bring its own border and layout
    <div role="group" aria-label={node.label} className="cw-sf-array">
      {nodes.map((child, i) =>
        child ? (
          <div key={i} className="cw-sf-array__item">
            <FieldView
              node={child}
              value={live.drafts[child.path] ?? items[i]}
              mixed={ctx.isMixed(child.path)}
              keyframe={undefined}
              hasKeyframes={false}
            />
            <IconButton
              size="sm"
              icon="x"
              label={strings.removeItem(strings.item(node.label, i))}
              disabled={ctx.disabled || !canRemove}
              onClick={() =>
                ctx.bus.commit(
                  node.path,
                  items.filter((_, j) => j !== i),
                )
              }
            />
          </div>
        ) : null,
      )}
      <Button
        size="sm"
        variant="ghost"
        icon="plus"
        disabled={ctx.disabled || !canAdd}
        onClick={() => ctx.bus.commit(node.path, [...items, seed])}
      >
        {strings.addItem(node.label)}
      </Button>
    </div>
  );
}
