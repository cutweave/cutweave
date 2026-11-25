// SPDX-License-Identifier: GPL-3.0-or-later
// The command palette (WP-2H.04), Ctrl+K (Cmd+K on macOS): one search box over every command the
// editor has (engine commands, editor commands, panels, workspaces, settings pages and rows).
// Type to filter (fuzzy), Up and Down to move, Enter to run, Esc to clear and then to close. A
// command that needs arguments the context cannot answer opens a small form generated from its
// input schema. A row has its own context menu (`palette.row`). The list and what a row does
// are `entries.ts`; this file is the view.
import { Button, Dialog, SearchField, Switch, TextField } from "@cutweave/ui";
import {
  type FormEvent,
  type KeyboardEvent,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useStore } from "zustand";
import type { TKey } from "../i18n/index.ts";
import { useContextMenu } from "../menus/host.tsx";
import { type Field, type FieldValues, fieldsOf, initialValues, toArguments } from "./argForm.ts";
import {
  buildEntries,
  type FormPlan,
  type PaletteEntry,
  type PaletteEnv,
  planFor,
  runEntry,
} from "./entries.ts";
import { appPaletteEnv } from "./env.ts";
import { closePalette, paletteStore } from "./paletteStore.ts";
import { recentsStore } from "./recents.ts";
import { rank } from "./search.ts";

/** Rows drawn at most; the rest are reached by typing. */
export const MAX_ROWS = 100;

type Mode =
  | { readonly kind: "list" }
  | { readonly kind: "form"; readonly entry: PaletteEntry; readonly plan: FormPlan };

const optionId = (base: string, key: string): string => `${base}-${key}`;

/** The palette's rows for a query: recent commands first with no query, else best match first. */
export function visibleEntries(
  entries: readonly PaletteEntry[],
  recent: readonly string[],
  query: string,
): { readonly rows: readonly PaletteEntry[]; readonly recentCount: number } {
  const order = new Map(recent.map((key, i) => [key, i]));
  const recents = entries
    .filter((e) => order.has(e.key))
    .sort((a, b) => (order.get(a.key) ?? 0) - (order.get(b.key) ?? 0));
  const rest = entries.filter((e) => !order.has(e.key));
  const base = [...recents, ...rest];
  if (query.trim() === "") return { rows: base, recentCount: recents.length };
  const ranked = rank(
    base.map((e) => e.search),
    query,
  );
  return { rows: ranked.map((i) => base[i] as PaletteEntry), recentCount: 0 };
}

interface RowProps {
  readonly entry: PaletteEntry;
  readonly id: string;
  readonly active: boolean;
  readonly onChoose: () => void;
  readonly onHover: () => void;
  readonly focusInput: () => void;
}

function Row({ entry, id, active, onChoose, onHover, focusInput }: RowProps) {
  const ref = useContextMenu<HTMLDivElement>("palette.row", () => ({
    ids: [],
    name: entry.title,
    data: { key: entry.key, command: entry.command },
  }));
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: the search field owns the keys (aria-activedescendant)
    <div
      ref={ref}
      id={id}
      role="option"
      aria-selected={active}
      aria-disabled={entry.disabled !== undefined ? true : undefined}
      tabIndex={-1}
      className={`cw-palette__row${active ? " cw-palette__row--active" : ""}${
        entry.disabled !== undefined ? " cw-palette__row--disabled" : ""
      }`}
      onMouseMove={onHover}
      onClick={onChoose}
      onFocus={focusInput}
    >
      <span className="cw-palette__main">
        <span className="body">{entry.title}</span>
        {entry.chips.map((chip) => (
          <span key={chip} className="cw-palette__chip caption">
            {chip}
          </span>
        ))}
      </span>
      {entry.disabled !== undefined || entry.summary !== undefined ? (
        <span className="cw-palette__summary caption">{entry.disabled ?? entry.summary}</span>
      ) : null}
      {entry.shortcut !== undefined ? (
        <kbd className="cw-kbd numeric cw-palette__keys">{entry.shortcut}</kbd>
      ) : null}
    </div>
  );
}

interface FormProps {
  readonly entry: PaletteEntry;
  readonly plan: FormPlan;
  readonly onRun: (args: Record<string, unknown>) => void;
  readonly onBack: () => void;
}

/** The argument form of a command that needs them (generated from its input schema). */
function ArgumentForm({ entry, plan, onRun, onBack }: FormProps) {
  const { t } = useTranslation();
  const fields = useMemo(() => fieldsOf(plan.schema), [plan.schema]);
  const [values, setValues] = useState<FieldValues>(() => initialValues(fields, plan.prefilled));
  const [errors, setErrors] = useState<Readonly<Record<string, TKey>>>({});
  const base = useId();
  const first = useRef<HTMLElement | null>(null);

  useEffect(() => first.current?.focus(), []);

  const set = (name: string, value: string | boolean): void =>
    setValues((v) => ({ ...v, [name]: value }));

  const submit = (event: FormEvent): void => {
    event.preventDefault();
    const result = toArguments(fields, values);
    if (!result.ok) {
      setErrors(result.errors);
      return;
    }
    onRun({ ...result.args, ...(entry.args ?? {}) });
  };

  const control = (f: Field, id: string, index: number) => {
    const ref = (el: HTMLElement | null): void => {
      if (index === 0) first.current = el;
    };
    const describedBy = [
      f.description !== undefined ? `${id}-help` : null,
      errors[f.name] !== undefined ? `${id}-error` : null,
    ]
      .filter((x): x is string => x !== null)
      .join(" ");
    const v = values[f.name];
    if (f.kind === "boolean") {
      return (
        <Switch
          id={id}
          checked={v === true}
          onCheckedChange={(on) => set(f.name, on)}
          aria-labelledby={`${id}-label`}
        />
      );
    }
    if (f.kind === "select") {
      return (
        <select
          id={id}
          ref={ref}
          className="cw-field body"
          value={typeof v === "string" ? v : ""}
          aria-describedby={describedBy || undefined}
          onChange={(event) => set(f.name, event.target.value)}
        >
          <option value="">{t("palette.form.choose")}</option>
          {f.options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      );
    }
    return (
      <TextField
        id={id}
        ref={ref}
        value={typeof v === "string" ? v : ""}
        onChange={(next) => set(f.name, next)}
        invalid={errors[f.name] !== undefined}
        numeric={f.kind === "number"}
        aria-describedby={describedBy || undefined}
        aria-required={f.required ? true : undefined}
      />
    );
  };

  return (
    <form className="cw-palette__form" onSubmit={submit} noValidate>
      <h3 className="title cw-palette__formtitle">{entry.title}</h3>
      {entry.summary !== undefined ? <p className="cw-note">{entry.summary}</p> : null}
      {fields.map((f, index) => {
        const id = `${base}-${f.name}`;
        return (
          <div key={f.name} className="cw-palette__field">
            <label id={`${id}-label`} htmlFor={id} className="label">
              {f.label}
              {f.required ? null : (
                <span className="cw-palette__optional caption"> {t("palette.form.optional")}</span>
              )}
            </label>
            {control(f, id, index)}
            {f.description !== undefined ? (
              <p id={`${id}-help`} className="cw-note">
                {f.description}
              </p>
            ) : null}
            {errors[f.name] !== undefined ? (
              <p id={`${id}-error`} className="cw-note cw-palette__error" role="alert">
                {t(errors[f.name] as TKey)}
              </p>
            ) : null}
          </div>
        );
      })}
      <div className="cw-dialog__footer">
        <Button variant="ghost" size="lg" onClick={onBack}>
          {t("palette.form.back")}
        </Button>
        <Button variant="primary" size="lg" type="submit">
          {t("palette.form.run")}
        </Button>
      </div>
    </form>
  );
}

export interface CommandPaletteProps {
  /** Where the list comes from; the app's by default. */
  readonly env?: PaletteEnv;
}

/** Mounted once in `EditorShell`. */
export function CommandPalette({ env: given }: CommandPaletteProps) {
  const { t } = useTranslation();
  const open = useStore(paletteStore, (s) => s.open);
  const openCount = useStore(paletteStore, (s) => s.openCount);
  const pendingRun = useStore(paletteStore, (s) => s.pendingRun);
  const recent = useStore(recentsStore, (s) => s.keys);
  const env = useMemo(() => given ?? appPaletteEnv(), [given]);

  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [mode, setMode] = useState<Mode>({ kind: "list" });
  const input = useRef<HTMLInputElement>(null);
  const base = useId();
  const listId = `${base}-list`;

  // The list is built when the palette opens: the context (selection, playhead) is the moment's.
  const entries = useMemo(() => (open ? buildEntries(env) : []), [open, openCount, env]);
  const { rows, recentCount } = useMemo(
    () => visibleEntries(entries, recent, query),
    [entries, recent, query],
  );
  const shown = rows.slice(0, MAX_ROWS);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    setMode({ kind: "list" });
  }, [open, openCount]);

  useEffect(() => setActive(0), [query]);

  // Keep the active row in view.
  const activeKey = shown[active]?.key;
  useEffect(() => {
    if (activeKey === undefined) return;
    document.getElementById(optionId(base, activeKey))?.scrollIntoView?.({ block: "nearest" });
  }, [activeKey, base]);

  const execute = (entry: PaletteEntry, args: Record<string, unknown>): void => {
    recentsStore.getState().push(entry.key);
    closePalette();
    // After the palette has left and the focus is back, so what the command opens keeps it.
    setTimeout(() => {
      void runEntry(entry, args, {
        services: env.services,
        selection: () => env.prefill().items,
      }).catch((error: unknown) => env.services.report(error));
    }, 0);
  };

  const choose = (entry: PaletteEntry | undefined): void => {
    if (entry === undefined || entry.disabled !== undefined) return;
    const plan = planFor(entry, env);
    if (plan.kind === "run") execute(entry, plan.args);
    else setMode({ kind: "form", entry, plan: plan.plan });
  };

  // A row's menu asked to run it.
  useEffect(() => {
    if (pendingRun === null) return;
    paletteStore.getState().clearRun();
    choose(entries.find((e) => e.key === pendingRun));
  }, [pendingRun]);

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.nativeEvent.isComposing) return;
    const count = shown.length;
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        if (count > 0) setActive((i) => (i + 1) % count);
        break;
      case "ArrowUp":
        event.preventDefault();
        if (count > 0) setActive((i) => (i - 1 + count) % count);
        break;
      case "PageDown":
        event.preventDefault();
        setActive((i) => Math.min(count - 1, i + 8));
        break;
      case "PageUp":
        event.preventDefault();
        setActive((i) => Math.max(0, i - 8));
        break;
      case "Enter":
        event.preventDefault();
        choose(shown[active]);
        break;
    }
  };

  const back = (): void => {
    setMode({ kind: "list" });
    requestAnimationFrame(() => input.current?.focus());
  };

  const inForm = mode.kind === "form";
  const moreRows = rows.length - shown.length;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) closePalette();
      }}
      title={t("palette.title")}
      className="cw-dialog--palette"
      // Esc (or a click outside) steps back first: out of the form, then out of the typed text.
      dirty={inForm || query !== ""}
      onDismissAttempt={() => {
        if (inForm) back();
        else setQuery("");
      }}
    >
      {mode.kind === "form" ? (
        <ArgumentForm
          key={mode.entry.key}
          entry={mode.entry}
          plan={mode.plan}
          onRun={(args) => execute(mode.entry, args)}
          onBack={back}
        />
      ) : (
        <>
          <SearchField
            ref={input}
            value={query}
            onChange={setQuery}
            label={t("palette.search")}
            clearLabel={t("palette.clear")}
            count={t("palette.count", { count: rows.length })}
            onExit={closePalette}
            onKeyDown={onKeyDown}
            role="combobox"
            aria-expanded="true"
            aria-controls={listId}
            aria-autocomplete="list"
            aria-activedescendant={activeKey === undefined ? undefined : optionId(base, activeKey)}
          />
          <div
            id={listId}
            role="listbox"
            aria-label={t("palette.results")}
            className="cw-palette__list"
          >
            {recentCount > 0 ? (
              // biome-ignore lint/a11y/useSemanticElements: a listbox's groups are not form fieldsets
              <div role="group" aria-label={t("palette.recent")}>
                <div className="cw-palette__heading caption" aria-hidden="true">
                  {t("palette.recent")}
                </div>
                {shown.slice(0, recentCount).map((entry, i) => (
                  <Row
                    key={entry.key}
                    entry={entry}
                    id={optionId(base, entry.key)}
                    active={i === active}
                    onChoose={() => choose(entry)}
                    onHover={() => setActive(i)}
                    focusInput={() => input.current?.focus()}
                  />
                ))}
              </div>
            ) : null}
            {/* biome-ignore lint/a11y/useSemanticElements: a listbox's groups are not form fieldsets */}
            <div role="group" aria-label={t(recentCount > 0 ? "palette.all" : "palette.results")}>
              {recentCount > 0 ? (
                <div className="cw-palette__heading caption" aria-hidden="true">
                  {t("palette.all")}
                </div>
              ) : null}
              {shown.slice(recentCount).map((entry, j) => {
                const i = j + recentCount;
                return (
                  <Row
                    key={entry.key}
                    entry={entry}
                    id={optionId(base, entry.key)}
                    active={i === active}
                    onChoose={() => choose(entry)}
                    onHover={() => setActive(i)}
                    focusInput={() => input.current?.focus()}
                  />
                );
              })}
            </div>
          </div>
          {shown.length === 0 ? (
            <p className="cw-note cw-palette__empty">
              {t("palette.empty", { query: query.trim() })}
            </p>
          ) : null}
          {moreRows > 0 ? (
            <p className="cw-note cw-palette__more">{t("palette.more", { count: moreRows })}</p>
          ) : null}
        </>
      )}
    </Dialog>
  );
}
