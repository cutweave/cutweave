// SPDX-License-Identifier: GPL-3.0-or-later
// Test writer for the bridge e2e: the Rust `ring_fixture` example of crates/cutweave-bridge
// (built by `scripts/build-bridge.ts --fixture`, which `pnpm e2e` runs), a stand-in for the
// engine publishing frames through the real `FramePublisher`.

import { type ChildProcess, spawn } from "node:child_process";
import path from "node:path";
import { createInterface } from "node:readline";
import { desktopDir } from "./fixtures.ts";

const repoRoot = path.resolve(desktopDir, "..", "..");
const profile = process.env.CUTWEAVE_BRIDGE_PROFILE === "release" ? "release" : "debug";
const fixtureExe = path.join(
  process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, "target"),
  profile,
  "examples",
  process.platform === "win32" ? "ring_fixture.exe" : "ring_fixture",
);

export interface RingAnnouncement {
  event: string;
  name: string;
  epoch: string;
  pid: number;
  width: number;
  height: number;
  format: string;
}

export class RingFixture {
  private readonly events: RingAnnouncement[] = [];
  private waiters: (() => void)[] = [];

  private constructor(
    private readonly child: ChildProcess,
    readonly ready: RingAnnouncement,
  ) {}

  static async start(args: string[] = []): Promise<RingFixture> {
    const child = spawn(fixtureExe, args, { stdio: ["pipe", "pipe", "inherit"] });
    const lines = createInterface({ input: child.stdout as NodeJS.ReadableStream });
    const ready = await new Promise<RingAnnouncement>((resolve, reject) => {
      child.once("error", reject);
      child.once("exit", (code) => reject(new Error(`ring_fixture exited early (${code})`)));
      lines.once("line", (line) => resolve(JSON.parse(line) as RingAnnouncement));
    });
    const fixture = new RingFixture(child, ready);
    lines.on("line", (line) => {
      fixture.events.push(JSON.parse(line) as RingAnnouncement);
      for (const w of fixture.waiters.splice(0)) w();
    });
    return fixture;
  }

  send(command: string): void {
    this.child.stdin?.write(`${command}\n`);
  }

  async event(name: string): Promise<RingAnnouncement> {
    for (;;) {
      const hit = this.events.find((e) => e.event === name);
      if (hit) return hit;
      await new Promise<void>((resolve) => this.waiters.push(resolve));
    }
  }

  async stop(): Promise<void> {
    if (this.child.exitCode !== null) return;
    this.send("quit");
    await new Promise<void>((resolve) => {
      const t = setTimeout(() => {
        this.child.kill();
        resolve();
      }, 3000);
      this.child.once("exit", () => {
        clearTimeout(t);
        resolve();
      });
    });
  }
}
