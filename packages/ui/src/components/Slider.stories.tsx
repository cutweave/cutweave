// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import type { KeyframeState } from "./KeyframeToggle.tsx";
import { Slider } from "./Slider.tsx";

export default { title: "Inputs/Slider", component: Slider, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-2)",
  width: "var(--layer-width-md)",
} as const;

const noop = () => {};

function Demo({ kf }: { kf: KeyframeState }) {
  const [opacity, setOpacity] = useState(100);
  const [volume, setVolume] = useState(-6);
  const [blur, setBlur] = useState(0);
  return (
    <div style={col}>
      <Slider
        label="Opacity"
        value={opacity}
        min={0}
        max={100}
        format={(v) => `${v}%`}
        onChange={setOpacity}
        keyframe={{ state: "off", onToggle: noop }}
      />
      <Slider
        label="Volume"
        value={volume}
        min={-60}
        max={12}
        format={(v) => `${v} dB`}
        onChange={setVolume}
        keyframe={{ state: kf, onToggle: noop, onPrev: noop, onNext: noop }}
      />
      <Slider
        label="Blur"
        value={blur}
        min={0}
        max={50}
        format={(v) => `${v} px`}
        onChange={setBlur}
        disabled
        keyframe={{ state: "off", onToggle: noop, disabled: true }}
      />
      <Slider label="Feather" value={20} min={0} max={100} onChange={noop} />
    </div>
  );
}

export const KeyframeOnAtPlayhead = { render: () => <Demo kf="on" /> };
export const KeyframesElsewhere = { render: () => <Demo kf="elsewhere" /> };
