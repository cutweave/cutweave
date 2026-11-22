// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { ActorBadge } from "./ActorBadge.tsx";

afterEach(cleanup);

describe("ActorBadge", () => {
  it("shows You for the user, with no marigold or agent variant", () => {
    render(h(ActorBadge, { kind: "user" }));
    const el = screen.getByText("You");
    expect(el.className).toContain("cw-actor");
    expect(el.className).not.toContain("--agent");
    expect(el.className).not.toContain("--plugin");
  });

  it("keeps the agent id in the text and uses the agent variant with sparkles", () => {
    render(h(ActorBadge, { kind: "agent", id: "claude-code" }));
    const el = screen.getByText("Agent: claude-code");
    expect(el.className).toContain("cw-actor--agent");
    expect(el.querySelector("svg")?.getAttribute("data-icon")).toBe("sparkles");
  });

  it("keeps plugins neutral with the plugin id", () => {
    render(h(ActorBadge, { kind: "plugin", id: "acme.lut-pack" }));
    const el = screen.getByText("Plugin: acme.lut-pack");
    expect(el.className).toContain("cw-actor--plugin");
    expect(el.className).not.toContain("--agent");
  });

  it("icons are decorative because the text always names the actor", () => {
    render(
      h("ul", { role: "list" }, [
        h("li", { key: 1 }, h(ActorBadge, { kind: "user" })),
        h("li", { key: 2 }, h(ActorBadge, { kind: "plugin", id: "x" })),
      ]),
    );
    for (const svg of document.querySelectorAll("svg")) {
      expect(svg.getAttribute("aria-hidden")).toBe("true");
    }
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
  });
});
