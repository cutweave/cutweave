// SPDX-License-Identifier: GPL-3.0-or-later
import path from "node:path";
import { describe, expect, it } from "vitest";
import { baseResponseHeaders, registerAppProtocol, resolveAppRequest } from "./app-protocol.ts";

const root = path.resolve("/bundle/renderer");
const at = (p: string) => path.resolve(root, p);

describe("resolveAppRequest", () => {
  it("serves assets and falls back to index.html", () => {
    expect(resolveAppRequest(root, "app://cutweave/assets/app.js")).toBe(at("assets/app.js"));
    expect(resolveAppRequest(root, "app://cutweave/")).toBe(at("index.html"));
    expect(resolveAppRequest(root, "app://cutweave/some/route")).toBe(at("index.html"));
  });

  it("rejects other hosts, schemes and traversal", () => {
    expect(resolveAppRequest(root, "app://evil/index.html")).toBeNull();
    expect(resolveAppRequest(root, "file:///etc/passwd")).toBeNull();
    // WHATWG URL parsing already collapses %2e%2e segments, so this stays inside the root.
    expect(resolveAppRequest(root, "app://cutweave/%2e%2e/%2e%2e/secret.txt")).toBe(
      at("secret.txt"),
    );
    expect(resolveAppRequest(root, "app://cutweave/..%2f..%2fsecret.txt")).toBeNull();
    expect(resolveAppRequest(root, "app://cutweave/a%5c..%5cb.js")).toBeNull();
    expect(resolveAppRequest(root, "app://cutweave/a%00.js")).toBeNull();
    expect(resolveAppRequest(root, "not a url")).toBeNull();
  });
});

describe("registerAppProtocol", () => {
  const setup = (upstream: () => Promise<Response>) => {
    let handler: (r: { url: string }) => Promise<Response> | Response = () => new Response();
    registerAppProtocol(
      {
        handle: (_s, h) => {
          handler = h;
        },
      },
      { fetch: upstream },
      root,
      () => ({ ...baseResponseHeaders(), "X-Test": "1" }),
    );
    return (url: string) => handler({ url });
  };

  it("adds main-controlled headers and returns 403/404 for bad requests", async () => {
    const get = setup(async () => new Response("ok", { headers: { "content-type": "text/html" } }));
    const ok = await get("app://cutweave/");
    expect(ok.status).toBe(200);
    expect(ok.headers.get("x-test")).toBe("1");
    expect(ok.headers.get("x-content-type-options")).toBe("nosniff");
    expect((await get("app://cutweave/..%2fx.js")).status).toBe(403);
    const missing = setup(async () => {
      throw new Error("ENOENT");
    });
    expect((await missing("app://cutweave/a.js")).status).toBe(404);
  });
});
