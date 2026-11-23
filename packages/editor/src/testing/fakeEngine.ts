// SPDX-License-Identifier: GPL-3.0-or-later
// A recording stand-in for the engine connection, for the component tests (jsdom): the stores get
// an `rpc` whose calls are written down and answered from a table. The flows against a real
// engine are in `session.integration.test.ts`.

import type { RpcClient } from "@cutweave/protocol";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";

export interface RecordedCall {
  readonly name: string;
  readonly input: unknown;
  readonly meta: unknown;
}

export interface FakeEngine {
  readonly calls: RecordedCall[];
  /** Names of the calls, in order. */
  names(): string[];
  /** Answers `name` with `answer(input)`; the default answer is `{}`. */
  answer(name: string, answer: (input: unknown) => unknown): void;
  /** Rejects `name` once with the given error. */
  reject(name: string, error: Error): void;
  /** Puts the fake into `engineStore` as the live connection. */
  install(): void;
  uninstall(): void;
}

let txCounter = 0;

export function createFakeEngine(): FakeEngine {
  const calls: RecordedCall[] = [];
  const answers = new Map<string, (input: unknown) => unknown>([
    ["edit.begin_tx", () => ({ tx: `tx_${++txCounter}` })],
    ["edit.commit_tx", () => ({ changed: true })],
  ]);
  const rejections = new Map<string, Error>();
  const rpc = {
    call(name: string, input: unknown, meta?: unknown): Promise<unknown> {
      calls.push({ name, input, meta });
      const error = rejections.get(name);
      if (error !== undefined) {
        rejections.delete(name);
        return Promise.reject(error);
      }
      return Promise.resolve(answers.get(name)?.(input) ?? {});
    },
  } as unknown as RpcClient;
  return {
    calls,
    names: () => calls.map((c) => c.name),
    answer: (name, answer) => void answers.set(name, answer),
    reject: (name, error) => void rejections.set(name, error),
    install: () => engineStore.setState({ ...initialEngineState, rpc, connection: "ready" }),
    uninstall: () => engineStore.setState({ ...initialEngineState }),
  };
}
