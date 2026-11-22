// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { CostChip, formatCost } from "./CostChip.tsx";

afterEach(cleanup);

function chip(props: Parameters<typeof CostChip>[0]) {
  const { container } = render(h(CostChip, props));
  return container.firstElementChild as HTMLElement;
}

describe("CostChip", () => {
  it("prefixes estimates with a tilde and shows the amount in mono", () => {
    expect(formatCost(0.4)).toBe("~$0.40");
    const el = chip({ state: "estimate", amount: 0.4 });
    expect(el.textContent).toBe("~$0.40");
    expect(el.querySelector(".cw-mono")?.textContent).toBe("~$0.40");
    expect(el.hasAttribute("data-state")).toBe(false);
  });

  it("formats other currencies", () => {
    expect(formatCost(2.1, "EUR")).toBe("~€2.10");
  });

  it("free shows a check icon and words", () => {
    const el = chip({ state: "free" });
    expect(el.textContent).toBe("Free (local)");
    expect(el.querySelector("svg")?.getAttribute("data-icon")).toBe("check");
  });

  it("over budget is warning tone with an alert icon, the amount and words", () => {
    const el = chip({ state: "over-budget", amount: 2.1 });
    expect(el.getAttribute("data-state")).toBe("warning");
    expect(el.textContent).toBe("Over budget~$2.10");
    expect(el.querySelector("svg")?.getAttribute("data-icon")).toBe("triangle-alert");
  });

  it("approval is agent styled with sparkles and words", () => {
    const el = chip({ state: "approval", amount: 2.1 });
    expect(el.getAttribute("data-state")).toBe("approval");
    expect(el.textContent).toContain("Needs approval");
    expect(el.querySelector("svg")?.getAttribute("data-icon")).toBe("sparkles");
  });

  it("estimating says so in words", () => {
    expect(chip({ state: "estimating" }).textContent).toBe("Estimating");
  });
});
