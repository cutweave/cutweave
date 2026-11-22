// MUST FAIL to typecheck (ops/tests/typescript.rs asserts TS2353 on `position`):
// `clip.split` takes `{ item, at }`, so a misspelled field is a compile error, not a runtime one.
import type { CutweaveClient } from "../src/generated/commands.ts";

export async function wrongField(client: CutweaveClient): Promise<void> {
  await client.call("clip.split", { item: "itm_00000000000000000000000000000000", position: 5 });
}
