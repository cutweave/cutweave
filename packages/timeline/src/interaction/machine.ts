// SPDX-License-Identifier: GPL-3.0-or-later
// `createTimelineMachine(deps)`: the transition function with its state, listeners and animation
// frames. `send` runs one event; listeners get the intents it produced. While the state has
// coalesced engine traffic waiting (a gesture update, a scrub seek) or is auto-scrolling, one
// frame is requested; the frame sends a `frame` event, so a drag costs at most one engine update
// per frame however many pointer events arrive. Events sent from a listener run after the current
// one, in order.
import { type ChartEdge, transition, wantsFrame } from "./reducer.ts";
import { initialMachineState, type MachineState } from "./state.ts";
import type {
  FrameScheduler,
  MachineDeps,
  MachineEnv,
  MachineEvent,
  MachineIntent,
} from "./types.ts";

export type MachineListener = (intents: readonly MachineIntent[], state: MachineState) => void;

export interface TimelineMachine {
  /** Runs one event (and any a listener sends meanwhile). */
  send(event: MachineEvent): void;
  readonly state: MachineState;
  /** Called after every event that produced intents. Returns the unsubscribe function. */
  subscribe(listener: MachineListener): () => void;
  /** Called with every chart edge taken (debugging, the coverage check). */
  onEdge(listener: (edge: ChartEdge) => void): () => void;
  /** Rolls back any gesture in progress (as a lost window does) and stops the frames. */
  dispose(): void;
}

export interface TimelineMachineDeps extends MachineDeps {
  /** Animation frames of the canvas's window (default: a 16 ms timer). */
  readonly frames?: FrameScheduler;
}

const FRAME_MS = 16;

const timerFrames: FrameScheduler = {
  request: (cb) => setTimeout(cb, FRAME_MS),
  cancel: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export function createTimelineMachine(
  deps: TimelineMachineDeps,
  env: Partial<MachineEnv> = {},
): TimelineMachine {
  const frames = deps.frames ?? timerFrames;
  let state = initialMachineState(env);
  const listeners = new Set<MachineListener>();
  const edgeListeners = new Set<(edge: ChartEdge) => void>();
  const queue: MachineEvent[] = [];
  let running = false;
  let frame: unknown = null;
  let disposed = false;

  const schedule = (): void => {
    if (disposed || frame !== null || !wantsFrame(state)) return;
    frame = frames.request(() => {
      frame = null;
      run({ type: "frame" });
    });
  };

  const run = (event: MachineEvent): void => {
    queue.push(event);
    if (running) return;
    running = true;
    try {
      for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
        const out = transition(state, next, deps);
        state = out.state;
        for (const edge of out.edges) for (const l of edgeListeners) l(edge);
        if (out.intents.length > 0) for (const l of listeners) l(out.intents, state);
      }
    } finally {
      running = false;
    }
    schedule();
  };

  return {
    send(event) {
      if (!disposed) run(event);
    },
    get state() {
      return state;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    onEdge(listener) {
      edgeListeners.add(listener);
      return () => edgeListeners.delete(listener);
    },
    dispose() {
      if (disposed) return;
      run({ type: "blur" });
      disposed = true;
      if (frame !== null) frames.cancel(frame);
      frame = null;
      listeners.clear();
      edgeListeners.clear();
    },
  };
}
