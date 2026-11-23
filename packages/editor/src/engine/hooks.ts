// SPDX-License-Identifier: GPL-3.0-or-later
// `useCommand` and `useQuery`: how components talk to the engine. Every edit is a command (arch
// §5.1); the UI never changes `docStore`, it waits for the `doc.patched` that follows. No
// optimistic updates in v1 (WP-1M.02 out of scope).

import {
  type CommandMap,
  type QueryMap,
  type RequestMeta,
  type RpcClient,
  RpcError,
} from "@cutweave/protocol";
import { useCallback, useEffect, useRef, useState } from "react";
import { useDocStore } from "../state/docStore.ts";
import { engineStore, useEngineStore } from "./engineStore.ts";

export type CommandName = keyof CommandMap;
export type QueryName = keyof QueryMap;

export type CallStatus = "idle" | "pending" | "success" | "error";

/** Why edits are refused right now, or `null` (WP-1L.03: engine failed, schema mismatch). */
export function editBlockedReason(): string | null {
  const blocked = engineStore.getState().blocked;
  if (blocked === null) return null;
  return blocked.kind === "schema-mismatch"
    ? "this editor and the engine use different command schemas"
    : "the engine stopped; editing resumes when it restarts";
}

/** The live client, or a rejection a caller can show ("no engine connected"). */
function requireRpc(): RpcClient {
  const rpc = engineStore.getState().rpc;
  if (rpc === null) {
    throw new RpcError("unsupported", "no engine is connected");
  }
  const blocked = editBlockedReason();
  if (blocked !== null) {
    throw new RpcError("unsupported", blocked);
  }
  return rpc;
}

/** Runs a command outside React (menu handlers, shortcuts). Rejects with {@link RpcError}. */
export function runCommand<K extends CommandName>(
  name: K,
  input: CommandMap[K]["input"],
  meta?: RequestMeta,
): Promise<CommandMap[K]["output"]> {
  try {
    return requireRpc().call(name, input, meta);
  } catch (error) {
    return Promise.reject(error);
  }
}

export interface UseCommand<K extends CommandName> {
  /** Sends the command. Resolves with its output; rejects with the engine's {@link RpcError}. */
  run(input: CommandMap[K]["input"], meta?: RequestMeta): Promise<CommandMap[K]["output"]>;
  readonly status: CallStatus;
  readonly error: RpcError | null;
  /** The output of the latest successful call. */
  readonly result: CommandMap[K]["output"] | null;
  reset(): void;
}

/**
 * A command as a hook: `const split = useCommand("clip.split"); split.run({ item, at })`.
 * Names, inputs and outputs are typed from the generated schema. The latest call decides
 * `status`, so a slow earlier answer cannot overwrite a newer one.
 */
export function useCommand<K extends CommandName>(name: K): UseCommand<K> {
  const [state, setState] = useState<{
    status: CallStatus;
    error: RpcError | null;
    result: CommandMap[K]["output"] | null;
  }>({ status: "idle", error: null, result: null });
  const latest = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const run = useCallback(
    (input: CommandMap[K]["input"], meta?: RequestMeta) => {
      const call = ++latest.current;
      setState((s) => ({ ...s, status: "pending", error: null }));
      return runCommand(name, input, meta).then(
        (result) => {
          if (mounted.current && call === latest.current) {
            setState({ status: "success", error: null, result });
          }
          return result;
        },
        (error: unknown) => {
          const failure =
            error instanceof RpcError ? error : new RpcError("internal", String(error));
          if (mounted.current && call === latest.current) {
            setState((s) => ({ ...s, status: "error", error: failure }));
          }
          throw failure;
        },
      );
    },
    [name],
  );
  const reset = useCallback(() => {
    latest.current += 1;
    setState({ status: "idle", error: null, result: null });
  }, []);
  return { run, reset, ...state };
}

export interface UseQuery<K extends QueryName> {
  readonly data: QueryMap[K]["output"] | null;
  readonly error: RpcError | null;
  readonly status: "idle" | "loading" | "success" | "error";
  refetch(): void;
}

/**
 * A read-only query as a hook. It runs when the engine connects, when the input changes and
 * again after every document revision (queries read the live document), keeping the previous
 * `data` on screen meanwhile. `enabled: false` pauses it.
 */
export function useQuery<K extends QueryName>(
  name: K,
  input: QueryMap[K]["input"],
  options: { enabled?: boolean } = {},
): UseQuery<K> {
  const rpc = useEngineStore((s) => s.rpc);
  const connected = useEngineStore((s) => s.connection === "ready");
  const rev = useDocStore((s) => s.rev);
  const enabled = options.enabled ?? true;
  const key = JSON.stringify(input);
  const [state, setState] = useState<Omit<UseQuery<K>, "refetch">>({
    data: null,
    error: null,
    status: "idle",
  });
  const [nonce, setNonce] = useState(0);
  // `key` stands for `input`: an equal-by-value input from a new object must not refetch.
  const inputRef = useRef(input);
  inputRef.current = input;

  // `key` stands for `input`; `rev` and `nonce` are refetch triggers.
  useEffect(() => {
    if (!enabled || rpc === null || !connected) {
      return;
    }
    let cancelled = false;
    setState((s) => ({ ...s, status: s.data === null ? "loading" : s.status }));
    rpc.call(name, inputRef.current).then(
      (data) => {
        if (!cancelled) setState({ data, error: null, status: "success" });
      },
      (error: unknown) => {
        if (!cancelled) {
          const failure =
            error instanceof RpcError ? error : new RpcError("internal", String(error));
          setState((s) => ({ ...s, error: failure, status: "error" }));
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [rpc, connected, enabled, name, key, rev, nonce]);

  const refetch = useCallback(() => setNonce((n) => n + 1), []);
  return { ...state, refetch };
}
