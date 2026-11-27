// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { TextArea } from "./TextArea.tsx";

export default { title: "Inputs/TextArea", component: TextArea, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  width: "var(--layer-width-sm)",
  background: "var(--surface-1)",
} as const;

interface FieldProps {
  id: string;
  initial: string;
  invalid?: boolean;
  disabled?: boolean;
}

function Field({ id, initial, invalid, disabled }: FieldProps) {
  const [value, setValue] = useState(initial);
  return (
    <>
      <label className="label" htmlFor={id} style={{ color: "var(--text-secondary)" }}>
        Note
      </label>
      <TextArea
        id={id}
        value={value}
        onChange={setValue}
        placeholder="Add a note"
        invalid={invalid}
        disabled={disabled}
      />
    </>
  );
}

/** An asset's note: empty with its placeholder, and with two lines of text. */
export const Note = {
  render: () => (
    <div style={col}>
      <Field id="note-empty" initial="" />
      <Field id="note-text" initial={"Second take is the good one.\nSound starts late."} />
    </div>
  ),
};

export const InvalidAndDisabled = {
  render: () => (
    <div style={col}>
      <Field id="note-invalid" initial="Too long to store." invalid />
      <Field id="note-disabled" initial="The engine is not running." disabled />
    </div>
  ),
};
