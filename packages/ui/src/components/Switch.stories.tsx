// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { Switch } from "./Switch.tsx";

export default { title: "Inputs/Switch", component: Switch, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  width: "var(--layer-width-sm)",
} as const;

function Row({
  id,
  label,
  ...props
}: {
  id: string;
  label: string;
  initial: boolean;
  disabled?: boolean | undefined;
}) {
  const [checked, setChecked] = useState(props.initial);
  return (
    <div className="cw-row">
      <label className="cw-row__label label" htmlFor={id}>
        {label}
      </label>
      <Switch id={id} checked={checked} onCheckedChange={setChecked} disabled={props.disabled} />
    </div>
  );
}

export const States = {
  render: () => (
    <div style={col}>
      <Row id="snap" label="Snap to clips" initial />
      <Row id="ripple" label="Auto-ripple" initial={false} />
      <Row id="burn" label="Burn in captions" initial disabled />
      <Row id="loop" label="Loop playback" initial={false} disabled />
    </div>
  ),
};
