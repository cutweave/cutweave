// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import {
  backoffPolicy,
  CLOSE_RESYNC_REQUIRED,
  type ConnectionEvent,
  ConnectionMachine,
  nextState,
  type SupervisorHint,
  supervisedPolicy,
} from "./connection-state.ts";

const lost = { type: "socket_lost", code: 1006, reason: "" } as const;

function run(events: ConnectionEvent[]): ConnectionMachine {
  const machine = new ConnectionMachine();
  for (const event of events) {
    machine.dispatch(event);
  }
  return machine;
}

describe("ConnectionMachine", () => {
  it("walks idle to ready", () => {
    const machine = run([{ type: "connect" }, { type: "socket_open" }, { type: "handshake_ok" }]);
    expect(machine.state).toBe("ready");
  });

  it("goes reconnecting on a loss and connecting on retry", () => {
    const machine = run([
      { type: "connect" },
      { type: "socket_open" },
      { type: "handshake_ok" },
      lost,
    ]);
    expect(machine.state).toBe("reconnecting");
    machine.dispatch({ type: "retry" });
    expect(machine.state).toBe("connecting");
  });

  it("a failed attempt (loss while connecting or handshaking) goes back to reconnecting", () => {
    expect(run([{ type: "connect" }, lost]).state).toBe("reconnecting");
    expect(run([{ type: "connect" }, { type: "socket_open" }, lost]).state).toBe("reconnecting");
  });

  it("give_up fails; connect restarts from failed", () => {
    const machine = run([{ type: "connect" }, lost, { type: "give_up" }]);
    expect(machine.state).toBe("failed");
    machine.dispatch({ type: "connect" });
    expect(machine.state).toBe("connecting");
  });

  it("close is final", () => {
    const machine = run([{ type: "connect" }, { type: "close" }]);
    expect(machine.state).toBe("closed");
    expect(machine.dispatch({ type: "connect" })).toBe(false);
    expect(machine.state).toBe("closed");
  });

  it("ignores events that do not apply and does not notify for them", () => {
    const machine = new ConnectionMachine();
    const listener = vi.fn();
    machine.onChange(listener);
    expect(machine.dispatch({ type: "handshake_ok" })).toBe(false);
    expect(machine.dispatch({ type: "retry" })).toBe(false);
    expect(listener).not.toHaveBeenCalled();
    machine.dispatch({ type: "connect" });
    expect(listener).toHaveBeenCalledWith("connecting", "idle");
  });

  it("nextState is a pure lookup", () => {
    expect(nextState("ready", "socket_lost")).toBe("reconnecting");
    expect(nextState("ready", "handshake_ok")).toBeUndefined();
  });
});

describe("backoffPolicy", () => {
  it("doubles from the base up to the cap", () => {
    const policy = backoffPolicy({ baseMs: 100, maxMs: 500 });
    const delays = [0, 1, 2, 3, 4].map((n) => policy.nextDelayMs(n, { code: 1006, reason: "" }));
    expect(delays).toEqual([100, 200, 400, 500, 500]);
  });

  it("retries at once after 4001, only for the first attempt", () => {
    const policy = backoffPolicy();
    const resync = { code: CLOSE_RESYNC_REQUIRED, reason: "resync_required" };
    expect(policy.nextDelayMs(0, resync)).toBe(0);
    expect(policy.nextDelayMs(1, resync)).toBe(500);
  });

  it("gives up after maxAttempts", () => {
    const policy = backoffPolicy({ maxAttempts: 2 });
    expect(policy.nextDelayMs(1, { code: 1006, reason: "" })).not.toBeNull();
    expect(policy.nextDelayMs(2, { code: 1006, reason: "" })).toBeNull();
  });
});

describe("supervisedPolicy", () => {
  const loss = { code: 1006, reason: "" };

  it("waits as long as the engine restarts, gives up when the supervisor did, else backs off", () => {
    let hint: SupervisorHint = "restarting";
    const policy = supervisedPolicy(() => hint, {
      base: backoffPolicy({ baseMs: 100, maxAttempts: 2 }),
      waitMs: 750,
    });
    // Far past the base policy's limit: still waiting for the restart.
    expect(policy.nextDelayMs(50, loss)).toBe(750);
    hint = "down";
    expect(policy.nextDelayMs(0, loss)).toBeNull();
    hint = "up";
    expect(policy.nextDelayMs(0, loss)).toBe(100);
    expect(policy.nextDelayMs(2, loss)).toBeNull();
    hint = "unknown";
    expect(policy.nextDelayMs(1, loss)).toBe(200);
  });
});
