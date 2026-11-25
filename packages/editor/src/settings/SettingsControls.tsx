// SPDX-License-Identifier: GPL-3.0-or-later
// The controls of the Settings dialog: one `SettingRow` per registered row, with the control its
// spec names. A row that edits a preference writes it through the preferences client; rows
// never touch a store, the engine or main themselves.

import type { PreferenceKey } from "@cutweave/preferences";
import { samePreferenceValue } from "@cutweave/preferences";
import { Button, IconButton, SegmentedControl, SettingRow, Switch, TextField } from "@cutweave/ui";
import { type ReactNode, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import {
  type ChoiceOption,
  type ControlSpec,
  rowDescription,
  rowLabel,
  type SettingRowDef,
  type SettingsContext,
} from "./registry.ts";
import type { TextPart } from "./search.ts";

const optionText = (t: (key: TKey) => string, option: ChoiceOption): string =>
  option.label !== undefined ? t(option.label) : (option.text ?? String(option.value));

/** Text with the matching parts in `<mark>`. */
export function Marked({ parts }: { parts: readonly TextPart[] }): ReactNode {
  return parts.map((part, index) =>
    part.match ? <mark key={index}>{part.text}</mark> : <span key={index}>{part.text}</span>,
  );
}

function NumberControl({
  id,
  label,
  value,
  min,
  max,
  unit,
  disabled,
  onCommit,
}: {
  id: string;
  label: string;
  value: number;
  min: number;
  max: number;
  unit?: string | undefined;
  disabled: boolean;
  onCommit: (value: number) => void;
}) {
  const [draft, setDraft] = useState(String(value));
  // Follow the stored value (a reset, an import, another window).
  useEffect(() => setDraft(String(value)), [value]);
  const parsed = draft.trim() === "" ? Number.NaN : Number(draft);
  const valid = Number.isInteger(parsed) && parsed >= min && parsed <= max;
  return (
    <>
      <TextField
        id={id}
        type="number"
        numeric
        min={min}
        max={max}
        aria-label={label}
        value={draft}
        disabled={disabled}
        invalid={!valid}
        onChange={(text) => {
          setDraft(text);
          const next = text.trim() === "" ? Number.NaN : Number(text);
          if (Number.isInteger(next) && next >= min && next <= max) onCommit(next);
        }}
        onBlur={() => setDraft(String(value))}
      />
      {unit ? <span className="caption">{unit}</span> : null}
    </>
  );
}

interface ControlProps {
  spec: ControlSpec;
  id: string;
  label: string;
  value: unknown;
  disabled: boolean;
  ctx: SettingsContext;
  write: (value: unknown) => void;
}

function Control({ spec, id, label, value, disabled, ctx, write }: ControlProps) {
  const { t } = ctx;
  switch (spec.kind) {
    case "switch":
      return (
        <Switch
          id={id}
          checked={value === true}
          disabled={disabled}
          onCheckedChange={(checked) => write(checked)}
        />
      );
    case "segmented":
      return (
        <SegmentedControl
          aria-label={label}
          value={value === undefined ? null : String(value)}
          disabled={disabled}
          onChange={(next) => {
            const option = spec.options.find((o) => String(o.value) === next);
            if (option !== undefined) write(option.value);
          }}
          options={spec.options.map((o) => ({
            value: String(o.value),
            label: optionText(t, o),
          }))}
        />
      );
    case "select":
      return (
        <select
          id={id}
          className="cw-field body"
          value={String(value)}
          disabled={disabled}
          onChange={(event) => {
            const option = spec.options.find((o) => String(o.value) === event.target.value);
            if (option !== undefined) write(option.value);
          }}
        >
          {spec.options.map((o) => (
            <option key={String(o.value)} value={String(o.value)}>
              {optionText(t, o)}
            </option>
          ))}
        </select>
      );
    case "number":
      return (
        <NumberControl
          id={id}
          label={label}
          value={typeof value === "number" ? value : spec.min}
          min={spec.min}
          max={spec.max}
          unit={spec.unit === undefined ? undefined : t(spec.unit)}
          disabled={disabled}
          onCommit={write}
        />
      );
    case "folder": {
      const picker = ctx.desktop?.dialog;
      return (
        <>
          <TextField
            id={id}
            aria-label={label}
            value={typeof value === "string" ? value : ""}
            readOnly
            title={typeof value === "string" ? value : undefined}
            onChange={() => undefined}
          />
          <Button
            size="sm"
            disabled={disabled || picker === undefined}
            onClick={() => {
              void picker?.openProjectFolder().then((chosen) => {
                if (chosen !== null) write(chosen);
              });
            }}
          >
            {t(spec.choose)}
          </Button>
        </>
      );
    }
    case "list": {
      const items = spec.items(ctx);
      return (
        <div className="cw-settings__list">
          {items.length === 0 ? (
            <span className="caption">{t(spec.empty)}</span>
          ) : (
            <ul className="cw-settings__items" aria-label={label}>
              {items.map((item) => (
                <li key={item.id} className="cw-settings__item body">
                  <span className="cw-settings__itemlabel">{item.label}</span>
                  <IconButton
                    size="sm"
                    icon="x"
                    label={t(spec.removeLabel, { name: item.label })}
                    disabled={disabled}
                    onClick={() => void spec.remove(ctx, item.id)}
                  />
                </li>
              ))}
            </ul>
          )}
          {spec.add ? (
            <Button size="sm" disabled={disabled} onClick={() => void spec.add?.run(ctx)}>
              {t(spec.add.label)}
            </Button>
          ) : null}
        </div>
      );
    }
    case "action": {
      const status = spec.status?.(ctx);
      return (
        <>
          {status ? <span className="caption">{status}</span> : null}
          <Button
            size="sm"
            variant={spec.danger ? "danger" : "secondary"}
            disabled={disabled || spec.disabled?.(ctx) === true}
            onClick={() => void Promise.resolve(spec.run(ctx)).catch(() => undefined)}
          >
            {t(spec.label)}
          </Button>
        </>
      );
    }
    case "custom":
      return <>{spec.render(ctx, id)}</>;
  }
}

export interface SettingRowViewProps {
  row: SettingRowDef;
  ctx: SettingsContext;
  engineReady: boolean;
  highlighted?: boolean;
  labelParts?: readonly TextPart[];
  descriptionParts?: readonly TextPart[];
}

/** One registered row, as a `SettingRow` with its control, reset and condition. */
export function SettingRowView({
  row,
  ctx,
  engineReady,
  highlighted,
  labelParts,
  descriptionParts,
}: SettingRowViewProps) {
  const { t } = useTranslation();
  const key = row.key as PreferenceKey | undefined;
  const value = key === undefined ? undefined : ctx.values[key];
  const fallback = key === undefined ? undefined : ctx.client.store.getState().defaults[key];
  const modified = key !== undefined && !row.noRowReset && !samePreferenceValue(value, fallback);
  const needsEngine = row.applies === "engine" && !engineReady;
  const disabled = needsEngine || row.disabledWhen?.(ctx.values) === true;
  const note = needsEngine
    ? t("settings.needsEngine")
    : row.applies === "restart"
      ? t("settings.appliesOnRestart")
      : undefined;
  const label = rowLabel(row, t);
  const controlId = `${row.id}-control`;
  const labelsInput = ["switch", "select", "number", "folder"].includes(row.control.kind);
  const write = (next: unknown): void => {
    if (key === undefined) return;
    void ctx.client.set(key, next as never).catch(() => undefined);
  };
  return (
    <SettingRow
      id={row.id}
      label={label}
      {...(labelParts === undefined ? {} : { labelContent: <Marked parts={labelParts} /> })}
      description={rowDescription(row, t)}
      {...(descriptionParts === undefined
        ? {}
        : { descriptionContent: <Marked parts={descriptionParts} /> })}
      htmlFor={labelsInput ? controlId : undefined}
      modified={modified}
      onReset={
        key === undefined ? undefined : () => void ctx.client.reset([key]).catch(() => undefined)
      }
      resetLabel={t("settings.resetRow", { name: label })}
      note={note}
      disabled={disabled}
      highlighted={highlighted}
    >
      <Control
        spec={row.control}
        id={controlId}
        label={label}
        value={value}
        disabled={disabled}
        ctx={ctx}
        write={write}
      />
    </SettingRow>
  );
}
