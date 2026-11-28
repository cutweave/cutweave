// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's session state (WP-2C.07): what it shows (the viewport, which properties,
// absolute or normalized values) and how it edits (the tangent mode). View state, not project
// data: nothing here is saved with the project or sent to the engine. It lives outside the panel
// so closing and reopening the panel, or detaching it into its own window, keeps it. A store of
// its own: a pan or a zoom writes here, and nothing that reads the app's view state hears of it.
import type { GraphViewport, TangentMode } from "@cutweave/ui";
import { createStore } from "zustand/vanilla";

export interface GraphViewState {
  /** The time and value window shown; `null`: fit the curves. */
  readonly viewport: GraphViewport | null;
  /** The clips `viewport` was chosen for (their ids, joined): other clips are fitted afresh. */
  readonly shown: string;
  /** Each curve runs from its lowest key to its highest, so different units share the graph. */
  readonly normalized: boolean;
  readonly tangentMode: TangentMode;
  /** The properties not shown (`transform.position`), whichever clip they are of. */
  readonly hidden: readonly string[];
  /**
   * The components of the selected keys that are selected in the graph, as `prop|component`
   * (a position key is one key with two values: selecting its X point leaves Y alone). `null`:
   * every component (the keys were selected elsewhere, on the timeline's lanes say).
   */
  readonly mask: readonly string[] | null;
}

const NO_PROPS: readonly string[] = [];

export const initialGraphView: GraphViewState = {
  viewport: null,
  shown: "",
  normalized: false,
  tangentMode: "free",
  hidden: NO_PROPS,
  mask: null,
};

export const graphViewStore = createStore<GraphViewState>(() => initialGraphView);

/** Shows or hides a property's curves. */
export function toggleHidden(prop: string): void {
  graphViewStore.setState((s) => ({
    hidden: s.hidden.includes(prop) ? s.hidden.filter((p) => p !== prop) : [...s.hidden, prop],
  }));
}

/** A property the curve list names. */
export interface ShellProp {
  /** `item|prop`: one row per property of a clip. */
  readonly id: string;
  readonly prop: string;
  readonly label: string;
  /** The colour indexes of its curves (one, or two for a position). */
  readonly colors: readonly number[];
  readonly hidden: boolean;
}

/** The selected key in numbers, for the toolbar's Time and Value fields. */
export interface ShellReadout {
  /** How many keys are selected. */
  readonly count: number;
  /** The one selected key's sequence time (flicks), or null (none, or several times). */
  readonly time: number | null;
  /** The selected points' value when they agree, or null. */
  readonly value: number | null;
  /** The interpolation kind of the selected keys when they agree. */
  readonly kind: "hold" | "linear" | "bezier" | "spring" | null;
  /** The keys can be edited (their track is not locked). */
  readonly editable: boolean;
  /** A flip only holds. */
  readonly holdOnly: boolean;
}

/**
 * What the panel's React shell shows. The controller writes a new object only when something
 * the shell shows changed, so a document patch that moves a clip renders nothing.
 */
export interface GraphShellState {
  readonly props: readonly ShellProp[];
  readonly readout: ShellReadout;
  /** Bumped when the Time and Value fields must drop what was typed (a refused entry). */
  readonly epoch: number;
}

export const NO_READOUT: ShellReadout = {
  count: 0,
  time: null,
  value: null,
  kind: null,
  editable: false,
  holdOnly: false,
};

const NO_SHELL_PROPS: readonly ShellProp[] = [];

export const initialGraphShell: GraphShellState = {
  props: NO_SHELL_PROPS,
  readout: NO_READOUT,
  epoch: 0,
};

export const graphShellStore = createStore<GraphShellState>(() => initialGraphShell);

/** Counts the times the curves were rebuilt (tests: never while the panel is closed). */
export const graphWork = { rebuilds: 0 };
