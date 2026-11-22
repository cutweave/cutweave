// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { SegmentedControl } from "./SegmentedControl.tsx";

export default {
  title: "Inputs/SegmentedControl",
  component: SegmentedControl,
  tags: ["autodocs"],
};

const pad = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-4)",
  alignItems: "flex-start",
} as const;

const RATIOS = [
  { value: "16:9", label: "16:9" },
  { value: "9:16", label: "9:16" },
  { value: "1:1", label: "1:1" },
  { value: "4:5", label: "4:5" },
];

function Demo() {
  const [ratio, setRatio] = useState("16:9");
  const [fit, setFit] = useState("fit");
  return (
    <div style={pad}>
      <SegmentedControl
        aria-label="Aspect ratio"
        options={RATIOS}
        value={ratio}
        onChange={setRatio}
      />
      <SegmentedControl
        aria-label="Scale mode"
        options={[
          { value: "fit", label: "Fit" },
          { value: "fill", label: "Fill" },
          { value: "stretch", label: "Stretch", disabled: true },
        ]}
        value={fit}
        onChange={setFit}
      />
    </div>
  );
}

export const States = { render: () => <Demo /> };
