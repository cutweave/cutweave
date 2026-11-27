// SPDX-License-Identifier: GPL-3.0-or-later
// "Height of new tracks" (Settings > Timeline, WP-2A.13). The engine makes a track at its default
// height; a command that made one is followed by `track.set_height` for each new track, inside
// the same transaction, so adding the track stays one undo step. Medium is the default height:
// nothing is sent and the command runs as it always did.
import type { RequestMeta, RpcClient, TxId } from "@cutweave/protocol";

type Call = (name: string, args: unknown, meta?: RequestMeta) => Promise<unknown>;

/** The tracks a command's result says it created (`track.add`, a placement on a new track). */
export function createdTracks(output: unknown): string[] {
  const out = output as
    | { created_tracks?: unknown; track?: unknown; track_count?: unknown }
    | null
    | undefined;
  if (Array.isArray(out?.created_tracks)) {
    return out.created_tracks.filter((id): id is string => typeof id === "string");
  }
  // `track.add` returns `{track, index, track_count}`; other results name tracks they only used.
  return typeof out?.track === "string" && typeof out.track_count === "number" ? [out.track] : [];
}

/** Gives `tracks` the height, inside transaction `tx`. A refusal is not the edit's: it is dropped. */
export async function setNewTrackHeights(
  rpc: RpcClient,
  tx: TxId,
  tracks: readonly string[],
  height: number | null,
): Promise<void> {
  if (height === null) return;
  const call = rpc.call.bind(rpc) as unknown as Call;
  for (const track of tracks) {
    await call("track.set_height", { track, height }, { tx }).catch(() => undefined);
  }
}

/**
 * Runs `name` and gives the tracks it created `height`, as one undo step named like the command
 * alone. `height` `null`: the plain call.
 */
export async function callWithNewTrackHeight(
  rpc: RpcClient,
  name: string,
  args: unknown,
  height: number | null,
): Promise<unknown> {
  const call = rpc.call.bind(rpc) as unknown as Call;
  if (height === null) return call(name, args);
  const { tx } = await rpc.call("edit.begin_tx", { kind: "batch", label: name });
  try {
    const out = await call(name, args, { tx });
    await setNewTrackHeights(rpc, tx, createdTracks(out), height);
    await rpc.call("edit.commit_tx", { tx });
    return out;
  } catch (error) {
    await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
    throw error;
  }
}
