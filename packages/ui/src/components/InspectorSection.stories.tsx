// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { InspectorRow, InspectorSection } from "./InspectorSection.tsx";
import { NumberScrubber } from "./NumberScrubber.tsx";

export default {
  title: "Structure/InspectorSection",
  component: InspectorSection,
  tags: ["autodocs"],
};

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  width: "var(--layer-width-md)",
  background: "var(--surface-0)",
} as const;

function TransformSection() {
  const [expanded, setExpanded] = useState(true);
  const [scale, setScale] = useState(112);
  const [rotation, setRotation] = useState(0);
  const modified = scale !== 100 || rotation !== 0;
  return (
    <InspectorSection
      label="Transform"
      expanded={expanded}
      onExpandedChange={setExpanded}
      modified={modified}
      onReset={() => {
        setScale(100);
        setRotation(0);
      }}
    >
      <InspectorRow label="Scale" modified={scale !== 100} onReset={() => setScale(100)}>
        <NumberScrubber
          aria-label="Scale"
          value={scale}
          onChange={setScale}
          unit="%"
          min={1}
          max={400}
        />
      </InspectorRow>
      <InspectorRow label="Rotation" modified={rotation !== 0} onReset={() => setRotation(0)}>
        <NumberScrubber
          aria-label="Rotation"
          value={rotation}
          onChange={setRotation}
          unit="deg"
          min={-360}
          max={360}
        />
      </InspectorRow>
      <InspectorRow label="Opacity">
        <span className="cw-mono numeric">100%</span>
      </InspectorRow>
    </InspectorSection>
  );
}

function CollapsedSection({ label }: { label: string }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <InspectorSection label={label} expanded={expanded} onExpandedChange={setExpanded}>
      <InspectorRow label="Hidden while collapsed">
        <span className="cw-mono numeric">0</span>
      </InspectorRow>
    </InspectorSection>
  );
}

export const ExpandedAndCollapsed = {
  render: () => (
    <div style={col}>
      <TransformSection />
      <CollapsedSection label="Crop" />
      <CollapsedSection label="Audio" />
    </div>
  ),
};
