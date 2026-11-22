// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { NumberScrubber } from "./NumberScrubber.tsx";

export default { title: "Inputs/NumberScrubber", component: NumberScrubber, tags: ["autodocs"] };

const row = {
  padding: "var(--space-4)",
  display: "flex",
  flexWrap: "wrap",
  gap: "var(--space-3)",
  alignItems: "center",
} as const;

function Demo() {
  const [x, setX] = useState(960);
  const [scale, setScale] = useState(100);
  const [gain, setGain] = useState(-12.5);
  return (
    <div style={row}>
      <NumberScrubber label="X" value={x} unit="px" onChange={setX} />
      <NumberScrubber label="Scale" value={scale} unit="%" min={0} max={400} onChange={setScale} />
      <NumberScrubber label="Gain" value={gain} unit="dB" step={0.5} onChange={setGain} />
      <NumberScrubber label="Scale" value={100} mixed unit="%" onChange={() => {}} />
      <NumberScrubber label="Rotation" value={0} unit="°" disabled onChange={() => {}} />
    </div>
  );
}

export const States = { render: () => <Demo /> };
