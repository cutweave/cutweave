// SPDX-License-Identifier: GPL-3.0-or-later
import { useEffect, useRef, useState } from "react";
import { Icon } from "../icons/index.ts";
import { TextField } from "./TextField.tsx";

export default { title: "Inputs/TextField", component: TextField, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  width: "var(--layer-width-sm)",
} as const;

interface FieldProps {
  id: string;
  label: string;
  initial: string;
  unit?: string;
  placeholder?: string;
  numeric?: boolean;
  invalid?: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  autoFocus?: boolean;
}

function Field({ id, label, initial, unit, autoFocus, ...rest }: FieldProps) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (autoFocus) ref.current?.focus();
  }, [autoFocus]);
  return (
    <div className="cw-row">
      <label className="cw-row__label label" htmlFor={id}>
        {label}
      </label>
      <span style={{ display: "inline-flex", alignItems: "center", gap: "var(--space-1)" }}>
        <TextField ref={ref} id={id} value={value} onChange={setValue} {...rest} />
        {unit ? (
          <span className="numeric" style={{ color: "var(--text-muted)" }}>
            {unit}
          </span>
        ) : null}
      </span>
    </div>
  );
}

export const Default = {
  render: () => (
    <div style={col}>
      <Field id="name" label="Name" initial="Interview A" />
      <Field id="mixed" label="Mixed" initial="" placeholder="Mixed" />
    </div>
  ),
};

export const WithLabelAndHelper = {
  render: () => (
    <div style={col}>
      <Field id="project" label="Project name" initial="My film" />
      <p className="cw-note">
        <Icon name="info" size="md" />
        <span>Cutweave creates a folder with this name.</span>
      </p>
    </div>
  ),
};

export const Focused = {
  render: () => (
    <div style={col}>
      <Field id="focus" label="Name" initial="Interview A" autoFocus />
    </div>
  ),
};

export const Invalid = {
  render: () => (
    <div style={col}>
      <Field id="bad" label="Start" initial="soon" numeric invalid />
      <p className="cw-note" role="alert">
        <Icon name="circle-alert" size="md" />
        <span>Not a time. Try 00:00:02:00, 2.5s or f:60.</span>
      </p>
    </div>
  ),
};

export const Disabled = {
  render: () => (
    <div style={col}>
      <Field id="off" label="Name" initial="Locked clip" disabled />
    </div>
  ),
};

export const ReadOnly = {
  render: () => (
    <div style={col}>
      <Field id="ro" label="File" initial="beach.mp4" readOnly />
    </div>
  ),
};

export const WithUnit = {
  render: () => (
    <div style={col}>
      <Field id="gain" label="Gain" initial="-6.0" unit="dB" numeric />
      <Field id="opacity" label="Opacity" initial="80" unit="%" numeric />
    </div>
  ),
};

export const Timecode = {
  render: () => (
    <div style={col}>
      <Field id="start" label="Start" initial="00:00:01:00" numeric />
      <Field id="duration" label="Duration" initial="00:00:04:00" numeric />
    </div>
  ),
};
