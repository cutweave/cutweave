// SPDX-License-Identifier: GPL-3.0-or-later
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  registerAppProtocol,
  registerPluginProtocolStub,
  resolveAppRequest,
} from "./app-protocol.ts";
import { appResponseHeaders, buildCsp } from "./csp.ts";

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
  const setup = (upstream: () => Promise<Response>, port = () => 43_117) => {
    let handler: (r: { url: string }) => Promise<Response> | Response = () => new Response();
    registerAppProtocol(
      {
        handle: (_s, h) => {
          handler = h;
        },
      },
      { fetch: upstream },
      root,
      () => appResponseHeaders(buildCsp(port())),
    );
    return (url: string) => handler({ url });
  };

  it("adds main-controlled headers and returns 403/404 for bad requests", async () => {
    const get = setup(async () => new Response("ok", { headers: { "content-type": "text/html" } }));
    const ok = await get("app://cutweave/");
    expect(ok.status).toBe(200);
    expect(ok.headers.get("x-content-type-options")).toBe("nosniff");
    expect(ok.headers.get("content-security-policy")).toContain(
      "connect-src http://127.0.0.1:43117",
    );
    expect((await get("app://cutweave/..%2fx.js")).status).toBe(403);
    const missing = setup(async () => {
      throw new Error("ENOENT");
    });
    expect((await missing("app://cutweave/a.js")).status).toBe(404);
  });

  it("builds the CSP per response, so a new engine port applies on the next load", async () => {
    let port = 43_117;
    const get = setup(
      async () => new Response("ok", { headers: { "content-security-policy": "default-src *" } }),
      () => port,
    );
    const first = (await get("app://cutweave/")).headers.get("content-security-policy");
    port = 50_001;
    const second = (await get("app://cutweave/")).headers.get("content-security-policy");
    expect(first).toContain("127.0.0.1:43117");
    expect(second).toContain("127.0.0.1:50001");
    expect(second).not.toContain("43117");
    expect(second).not.toContain("default-src *");
  });
});

describe("registerPluginProtocolStub", () => {
  it("claims the plugin scheme and fails closed with 404", async () => {
    const handled: Record<string, (r: { url: string }) => Promise<Response> | Response> = {};
    registerPluginProtocolStub({
      handle: (scheme, h) => {
        handled[scheme] = h;
      },
    });
    expect(Object.keys(handled)).toEqual(["cutweave-plugin"]);
    const res = await handled["cutweave-plugin"]?.({ url: "cutweave-plugin://p/index.html" });
    expect(res?.status).toBe(404);
  });
});
