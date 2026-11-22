// SPDX-License-Identifier: GPL-3.0-or-later
// Print Markdown tables from results/bench-*.json (WP-0C.02 informal numbers; WP-0C.04 owns
// the formal aggregation). Usage: node scripts/summarize-bench.mjs [file.json]
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "results");
const files = process.argv[2]
  ? [process.argv[2]]
  : readdirSync(dir)
      .filter((f) => f.startsWith("bench-") && f.endsWith(".json"))
      .sort()
      .map((f) => join(dir, f));
const f1 = (x) => (typeof x === "number" && Number.isFinite(x) ? x.toFixed(1) : "-");
const pp = (l) => (l ? `${f1(l.p50)} / ${f1(l.p95)}` : "-");
const table = (head, rows) => {
  console.log(`| ${head.join(" | ")} |`);
  console.log(`|${head.map(() => "---").join("|")}|`);
  for (const r of rows) console.log(`| ${r.join(" | ")} |`);
  console.log();
};

for (const file of files) {
  const r = JSON.parse(readFileSync(file, "utf8"));
  console.log(`### ${file.split(/[\\/]/).pop()} (${r.pass ? "PASS" : "FAIL"})\n`);
  table(
    ["format + upload (1920x1080)", "fps", "drops/min (gaps + engine)", "upload+draw CPU p50/p95 ms", "submit -> GPU done p50/p95 ms", "engine render -> present p50/p95 ms", "CPU % engine / renderer / GPU proc"],
    r.matrix.map((m) => [
      `${m.format} + ${m.upload}${m.otherEngines ? " (contended*)" : ""}`,
      f1(m.presentFps),
      `${f1(m.dropsPerMin)} (${f1(m.seqGapsPerMin)} + ${f1(m.engineDropsPerMin)})`,
      pp(m.uploadDrawMs),
      pp(m.gpuDoneMs),
      pp(m.renderToPresentMs),
      `${f1(m.cpu.engine)} / ${f1(m.cpu.renderer)} / ${f1(m.cpu.gpu)}`,
    ]),
  );
  const star = (p) => (p?.otherEngines ? " (contended*)" : "");
  const canvas = r.phases.find((p) => p.phase.startsWith("play-canvas"));
  if (canvas) {
    console.log(
      `Canvas-size playback${star(canvas)} (${canvas.snapshot.size}, ${r.fastest.format} + ${r.fastest.upload}): ` +
        `${f1(canvas.presentFps)} fps, ${f1(canvas.dropsPerMin)} drops/min, renderer CPU ${f1(canvas.cpuMeanPercentOfOneCore.renderer)} %.\n`,
    );
  }
  table(
    ["scrub config", "requests", "scrub-to-pixel p50/p95 ms", "full-res on screen p50/p95 ms", "low-res frames shown", "rAF wait p50/p95 ms", "lost"],
    r.scrub.variants.map((v, i) => [
      `${v.name}${star(r.phases.filter((p) => p.phase.startsWith("scrub-"))[i])}`,
      v.scrub.requests,
      pp(v.scrub.latency.firstPixelMs),
      pp(v.scrub.latency.fullPixelMs),
      v.scrub.lowresPresented,
      v.scrub.waitMs?.n ? pp(v.scrub.waitMs) : "-",
      v.scrub.lost,
    ]),
  );
  const z = r.resize;
  console.log(
    `Resize${star(r.phases.find((p) => p.phase.startsWith("resize-")))}: ${z.stage.resizes} canvas resizes, ${z.stage.blankResizes} blank, ` +
      `${z.captures.n} captures checked (${z.captures.blank} blank, ${z.captures.stale} discarded as stale), ` +
      `renegotiation p50/p95 ${f1(z.stage.renegotiateMs.p50)} / ${f1(z.stage.renegotiateMs.p95)} ms, ` +
      `steps renegotiated ${z.steps.filter((s) => s.ok).length}/${z.steps.length}, ${f1(z.fps)} fps during the sweep.\n`,
  );
  console.log(
    `Overlay: ${r.overlay.hitTests.filter((h) => h.ok).length}/${r.overlay.hitTests.length} hit tests, ` +
      `${r.overlay.interactions.filter((i) => i.ok).length}/${r.overlay.interactions.length} interactions, ` +
      `aligned every second: ${r.checks.overlayAlignedEverySecond}.\n`,
  );
  console.log(`Checks: ${Object.entries(r.checks).map(([k, v]) => `${k} ${v ? "ok" : "FAIL"}`).join(", ")}\n`);
  if (r.notes?.length) console.log(`Notes (* = another s1-engine ran during the phase): ${r.notes.join("; ")}\n`);
}
