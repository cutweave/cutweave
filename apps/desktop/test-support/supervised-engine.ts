// SPDX-License-Identifier: GPL-3.0-or-later
// A stand-in for Electron main in the orphan test (supervisor.integration.test.ts): starts the
// engine under an EngineSupervisor, prints `{"engine":<pid>}` once it is ready and then idles
// until it is killed. The engine must notice its parent's death (stdin EOF) and exit.
//   node supervised-engine.ts <engine-binary> <port>

import { EngineSupervisor } from "../src/main/engine/supervisor.ts";

const [command, port] = process.argv.slice(2);
if (command === undefined || port === undefined) process.exit(2);

const supervisor = new EngineSupervisor({
  launch: { command, args: ["engine"] },
  port: Number(port),
  extraArgs: ["--no-log"],
  log: () => {},
});
const status = await supervisor.start();
if (status.state !== "ready") {
  process.stdout.write(`${JSON.stringify({ error: status.message ?? status.state })}\n`);
  process.exit(1);
}
process.stdout.write(`${JSON.stringify({ engine: supervisor.pid })}\n`);
setInterval(() => {}, 60_000);
