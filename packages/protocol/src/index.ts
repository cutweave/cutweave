// SPDX-License-Identifier: GPL-3.0-or-later
export type {
  ConnectionHooks,
  ConnectionInfo,
  DisconnectInfo,
  ReconnectContext,
  RpcConnectionOptions,
  RpcServerHello,
  SocketFactory,
  Timers,
  WebSocketLike,
} from "./connection.ts";
export {
  errorFromWire,
  isTransactionBound,
  isTransactionLost,
  RpcConnection,
  TRANSACTION_LOST,
  tokenSubprotocol,
} from "./connection.ts";
export type {
  ConnectionEvent,
  ConnectionState,
  ReconnectPolicy,
  SocketLoss,
  StateListener,
  SupervisorHint,
} from "./connection-state.ts";
export {
  backoffPolicy,
  CLOSE_RESYNC_REQUIRED,
  ConnectionMachine,
  nextState,
  supervisedPolicy,
} from "./connection-state.ts";
export type { ApplyOutcome, ApplyResult, DocSink, DocState } from "./doc-mirror.ts";
export { applyDocEvent, DocMirror, docFromSnapshot, EMPTY_DOC } from "./doc-mirror.ts";
export type { DocSyncOptions } from "./doc-sync.ts";
export { DocSync } from "./doc-sync.ts";
export type { CmdErrorCode, CommandMap, QueryMap, RpcMap } from "./generated/commands.ts";
export type { EventMap } from "./generated/events.ts";
export { SCHEMA_HASH } from "./generated/schema-hash.ts";
export type * from "./generated/types.ts";
export { needsRequestId, READ_ONLY_METHODS } from "./mutating.ts";
export type { OutboxEntryView } from "./outbox.ts";
export { Outbox } from "./outbox.ts";
export type { Json } from "./patch.ts";
export { applyPatch, jsonEqual, PatchError } from "./patch.ts";
export type { RandomBytes } from "./request-id.ts";
export { uuidv7 } from "./request-id.ts";
export type { ProjectSnapshot, RecordedRequest, RequestMeta, Transport } from "./rpc.ts";
export { MockTransport, RpcClient, RpcError } from "./rpc.ts";
