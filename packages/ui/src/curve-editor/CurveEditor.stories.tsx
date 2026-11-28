// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { SAMPLE_PRESETS, sampleCurve } from "./fixtures.ts";
// Through the index, which brings the component's stylesheet.
import { CurveEditor, type CurveValue } from "./index.ts";

export default { title: "Inputs/CurveEditor", component: CurveEditor, tags: ["autodocs"] };

const frame = {
  padding: "var(--space-4)",
  width: "calc(var(--space-12) * 6)",
} as const;

function Demo({ initial, hold = false }: { initial: CurveValue; hold?: boolean }) {
  const [value, setValue] = useState<CurveValue>(initial);
  return (
    <div style={frame}>
      <CurveEditor
        value={value}
        onCommit={setValue}
        presets={SAMPLE_PRESETS}
        sample={sampleCurve}
        kinds={hold ? ["hold"] : undefined}
      />
    </div>
  );
}

export const Bezier = {
  render: () => <Demo initial={{ kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 }} />,
};

export const Overshoot = {
  render: () => <Demo initial={{ kind: "bezier", x1: 0.34, y1: 1.56, x2: 0.64, y2: 1 }} />,
};

export const Spring = {
  render: () => (
    <Demo initial={{ kind: "spring", stiffness: 180, damping: 12, mass: 1, bounce: false }} />
  ),
};

export const Linear = { render: () => <Demo initial={{ kind: "linear" }} /> };

/** A property that only holds (a flip): the other kinds are unavailable. */
export const HoldOnly = { render: () => <Demo initial={{ kind: "hold" }} hold /> };

export const Disabled = {
  render: () => (
    <div style={frame}>
      <CurveEditor
        value={{ kind: "bezier", x1: 0.42, y1: 0, x2: 0.58, y2: 1 }}
        onCommit={() => {}}
        presets={SAMPLE_PRESETS}
        sample={sampleCurve}
        disabled
      />
    </div>
  ),
};
