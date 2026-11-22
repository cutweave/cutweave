// Typechecked by ops/tests/typescript.rs under the repo's strict tsconfig.base.json.
// Proves the generated client gives compile-time checked args and precise result types.
import {
  CutweaveClient,
  FLICKS_PER_SECOND,
  type RpcReply,
  type RpcRequest,
  type Time,
  type Transport,
} from "./generated/commands.ts";

/** Records requests and replies with an empty result; enough to typecheck the client. */
class RecordingTransport implements Transport {
  readonly sent: RpcRequest[] = [];

  async send(request: RpcRequest): Promise<RpcReply> {
    this.sent.push(request);
    return { ok: true, rev: this.sent.length, result: {}, dry_run: false, deduplicated: false };
  }
}

const seconds = (s: number): Time => s * FLICKS_PER_SECOND;

/** The S5 agent task ("import, cut at 2 s, show me the frame at 3 s") through the typed client. */
export async function importCutRender(client: CutweaveClient): Promise<string> {
  const { media } = await client.call("media.import", { path: "/footage/interview.mp4" });
  const { item } = await client.call("timeline.insert", { media, at: 0 });
  const { left, right } = await client.call(
    "clip.split",
    { item, at: seconds(2) },
    { requestId: "0190f5c2-0000-7000-8000-000000000001", expectedRev: 2 },
  );
  await client.call("clip.trim", { item: right, end: seconds(8) });
  const timeline = await client.call("timeline.get", {});
  const frame = await client.call("render.frame", { at: seconds(3), width: 320 });
  const history = await client.call("edit.undo", {});
  return `${left} ${timeline.tracks.length} ${frame.mime} ${frame.data.length} ${history.rev}`;
}

/** Each line below must fail to compile; `@ts-expect-error` turns "it compiled" into an error. */
export function compileTimeChecks(client: CutweaveClient): void {
  const item = "itm_00000000000000000000000000000000";
  // @ts-expect-error unknown field: params are exact (Rust side: deny_unknown_fields)
  void client.call("clip.split", { item, at: 0, bogus: true });
  // @ts-expect-error missing required field `at`
  void client.call("clip.split", { item });
  // @ts-expect-error Time is a number of flicks, never a bigint (ADR-015)
  void client.call("clip.split", { item, at: 10n });
  // @ts-expect-error unknown command name
  void client.call("clip.explode", {});
  // @ts-expect-error results are precise: SplitClipOutput has no `middle`
  void client.call("clip.split", { item, at: 0 }).then((out) => out.middle);
  // Optional params may be omitted or null, matching serde's Option<T> + default.
  void client.call("timeline.insert", { media: "med_0", at: 0, track: null });
}

export const demo = (): Promise<string> =>
  importCutRender(new CutweaveClient(new RecordingTransport()));
