// SPDX-License-Identifier: GPL-3.0-or-later
import { useMemo, useState } from "react";
import { denseCurves, FIXTURE_OPTIONS, mixedCurves, positionCurves } from "./fixtures.ts";
// Through the index, which brings the component's stylesheet.
import { type GraphCurve, GraphEditor, type GraphPointRef, type TangentMode } from "./index.ts";

export default { title: "Inputs/GraphEditor", component: GraphEditor, tags: ["autodocs"] };

const frame = {
  display: "flex",
  width: "calc(var(--space-12) * 14)",
  height: "calc(var(--space-12) * 7)",
} as const;

function Demo({
  curves,
  initial = [],
  normalized = false,
  tangentMode = "free",
  playhead = 2000,
}: {
  curves: readonly GraphCurve[];
  initial?: readonly GraphPointRef[];
  normalized?: boolean;
  tangentMode?: TangentMode;
  playhead?: number;
}) {
  const [selection, setSelection] = useState<readonly GraphPointRef[]>(initial);
  const events = useMemo(
    () => ({
      select: (points: readonly GraphPointRef[]) => setSelection(points),
    }),
    [],
  );
  return (
    <div style={frame}>
      <GraphEditor
        options={FIXTURE_OPTIONS}
        curves={curves}
        selection={selection}
        playhead={playhead}
        normalized={normalized}
        tangentMode={tangentMode}
        events={events}
      />
    </div>
  );
}

/** A position animation: two components, three keys each. */
export const Position = { render: () => <Demo curves={positionCurves()} /> };

/** The middle key selected: its two bezier handles show. */
export const Handles = {
  render: () => <Demo curves={positionCurves()} initial={[{ curve: "position.x", index: 1 }]} />,
};

/** A hold, a spring and an overshooting bezier on one curve. */
export const HoldSpringOvershoot = { render: () => <Demo curves={mixedCurves()} /> };

/** Curves of different units share the graph: each runs from its lowest key to its highest. */
export const Normalized = {
  render: () => <Demo curves={[...positionCurves(), ...mixedCurves()]} normalized />,
};

/** Ten curves of five hundred keys each. */
export const Dense = { render: () => <Demo curves={denseCurves(10, 500)} playhead={40000} /> };

export const Empty = { render: () => <Demo curves={[]} /> };
