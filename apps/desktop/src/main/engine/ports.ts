// SPDX-License-Identifier: GPL-3.0-or-later
// Loopback port helpers for the engine's stable port (arch §6): main picks it at launch and
// keeps it across restarts while it stays free.

import net from "node:net";

/** Ask the OS for a free loopback port. */
export function pickFreePort(host = "127.0.0.1"): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, host, () => {
      const address = server.address();
      server.close(() => {
        if (address !== null && typeof address === "object") resolve(address.port);
        else reject(new Error("no port assigned"));
      });
    });
  });
}

/** Whether `port` can be bound on loopback right now (checked before every engine restart). */
export function isPortFree(port: number, host = "127.0.0.1"): Promise<boolean> {
  return new Promise((resolve) => {
    const server = net.createServer();
    server.unref();
    server.once("error", () => resolve(false));
    server.listen({ port, host, exclusive: true }, () => {
      server.close(() => resolve(true));
    });
  });
}
