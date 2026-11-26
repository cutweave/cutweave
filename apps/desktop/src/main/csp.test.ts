// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { appResponseHeaders, buildCsp } from "./csp.ts";

/** Parse a policy into `{ directive: sources[] }`. */
function directives(csp: string): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const part of csp.split(";")) {
    const [name, ...sources] = part.trim().split(/\s+/);
    if (name) out[name] = sources;
  }
  return out;
}

describe("buildCsp", () => {
  const csp = buildCsp(43_117);
  const d = directives(csp);

  it("matches the WP-1L.02 policy for the engine port", () => {
    expect(d).toEqual({
      "default-src": ["'none'"],
      "script-src": ["'self'", "'wasm-unsafe-eval'"],
      "style-src": ["'self'"],
      "img-src": ["'self'", "data:", "blob:", "http://127.0.0.1:43117"],
      "font-src": ["'self'"],
      "connect-src": ["http://127.0.0.1:43117", "ws://127.0.0.1:43117"],
      "media-src": ["blob:", "http://127.0.0.1:43117"],
      "frame-src": ["cutweave-plugin:"],
      "object-src": ["'none'"],
      "base-uri": ["'none'"],
      "form-action": ["'none'"],
      "frame-ancestors": ["'none'"],
    });
  });

  it("never allows eval, inline code or wildcards in production", () => {
    expect(csp.replace("'wasm-unsafe-eval'", "")).not.toMatch(
      /unsafe-|\*|https?:\/\/(?!127\.0\.0\.1:43117)|localhost/,
    );
    expect(csp).not.toMatch(/'unsafe-(eval|inline)'/);
  });

  it("allows 'wasm-unsafe-eval' in script-src and nowhere else (ADR-0033)", () => {
    expect(d["script-src"]).toContain("'wasm-unsafe-eval'");
    for (const [name, sources] of Object.entries(d)) {
      if (name !== "script-src") expect(sources).not.toContain("'wasm-unsafe-eval'");
    }
    // The module comes from the app origin (bundled into a script): no data:/blob: in script-src.
    expect(d["script-src"]).toEqual(["'self'", "'wasm-unsafe-eval'"]);
  });

  it("keeps the strict policy for everything else", () => {
    const strict = buildCsp(43_117, { wasm: false });
    expect(strict).not.toContain("wasm-unsafe-eval");
    expect(directives(strict)["script-src"]).toEqual(["'self'"]);
    // Dev server relaxations never bring it back for a strict page.
    expect(buildCsp(null, { wasm: false, devServerOrigin: "http://localhost:5173" })).not.toContain(
      "wasm-unsafe-eval",
    );
  });

  it("connects only to the engine origin", () => {
    for (const source of d["connect-src"] ?? []) {
      expect(source).toMatch(/^(http|ws):\/\/127\.0\.0\.1:43117$/);
    }
  });

  it("follows the port and drops the old one", () => {
    const next = buildCsp(50_001);
    expect(next).toContain("127.0.0.1:50001");
    expect(next).not.toContain("43117");
  });

  it("blocks all connections when the app runs engine-less", () => {
    const none = directives(buildCsp(null));
    expect(none["connect-src"]).toEqual(["'none'"]);
    expect(none["img-src"]).toEqual(["'self'", "data:", "blob:"]);
    expect(none["media-src"]).toEqual(["blob:"]);
  });

  it("rejects invalid ports", () => {
    for (const port of [0, -1, 65_536, 1.5, Number.NaN]) {
      expect(() => buildCsp(port)).toThrow(RangeError);
    }
  });

  it("relaxes only for the dev server, and only when asked", () => {
    const dev = directives(buildCsp(43_117, { devServerOrigin: "http://localhost:5173" }));
    expect(dev["script-src"]).toEqual([
      "'self'",
      "'wasm-unsafe-eval'",
      "http://localhost:5173",
      "'unsafe-inline'",
    ]);
    expect(dev["connect-src"]).toContain("ws://localhost:5173");
    expect(dev["script-src"]).not.toContain("'unsafe-eval'");
    expect(dev["default-src"]).toEqual(["'none'"]);
  });
});

describe("appResponseHeaders", () => {
  it("sets the CSP and the companion headers", () => {
    expect(appResponseHeaders("default-src 'none'")).toEqual({
      "Content-Security-Policy": "default-src 'none'",
      "X-Content-Type-Options": "nosniff",
      "Cross-Origin-Opener-Policy": "same-origin",
      "Referrer-Policy": "no-referrer",
      "Cache-Control": "no-cache",
    });
  });
});
