// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RatingStars } from "./RatingStars.tsx";

afterEach(cleanup);

const starLabel = (n: number) => (n === 1 ? "1 star" : `${n} stars`);

function Harness(props: {
  initial: number | null;
  onChange?: (v: number | null) => void;
  disabled?: boolean;
}) {
  const [value, setValue] = useState(props.initial);
  return h(RatingStars, {
    value,
    "aria-label": "Rating",
    starLabel,
    disabled: props.disabled,
    onChange: (v: number | null) => {
      setValue(v);
      props.onChange?.(v);
    },
  });
}

const checked = () =>
  screen
    .getAllByRole("radio")
    .filter((r) => r.getAttribute("aria-checked") === "true")
    .map((r) => r.getAttribute("aria-label"));
const filled = () =>
  screen.getAllByRole("radio").filter((r) => r.getAttribute("data-filled") === "true").length;

describe("RatingStars", () => {
  it("is a named radiogroup of five named stars, none checked while unrated", () => {
    render(h(Harness, { initial: null }));
    expect(screen.getByRole("radiogroup", { name: "Rating" })).toBeTruthy();
    expect(screen.getAllByRole("radio").map((r) => r.getAttribute("aria-label"))).toEqual([
      "1 star",
      "2 stars",
      "3 stars",
      "4 stars",
      "5 stars",
    ]);
    expect(checked()).toEqual([]);
    expect(filled()).toBe(0);
    // One tab stop: the first star while unrated.
    expect(screen.getAllByRole("radio").map((r) => r.getAttribute("tabindex"))).toEqual([
      "0",
      "-1",
      "-1",
      "-1",
      "-1",
    ]);
  });

  it("checks the rating's star, fills the stars up to it and makes it the tab stop", () => {
    render(h(Harness, { initial: 3 }));
    expect(checked()).toEqual(["3 stars"]);
    expect(filled()).toBe(3);
    expect(screen.getByRole("radio", { name: "3 stars" }).getAttribute("tabindex")).toBe("0");
  });

  it("sets on click and clears when the current star is clicked again", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: null, onChange }));
    await user.click(screen.getByRole("radio", { name: "4 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(4);
    expect(filled()).toBe(4);
    await user.click(screen.getByRole("radio", { name: "2 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(2);
    await user.click(screen.getByRole("radio", { name: "2 stars" }));
    expect(onChange).toHaveBeenLastCalledWith(null);
    expect(checked()).toEqual([]);
    expect(onChange).toHaveBeenCalledTimes(3);
  });

  it("is reachable by Tab and set by the arrow keys, Home, End and Delete", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: null, onChange }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "1 star" }));
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith(1);
    await user.keyboard("{ArrowRight}{ArrowUp}");
    expect(onChange).toHaveBeenLastCalledWith(3);
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "3 stars" }));
    await user.keyboard("{ArrowLeft}");
    expect(onChange).toHaveBeenLastCalledWith(2);
    await user.keyboard("{End}");
    expect(onChange).toHaveBeenLastCalledWith(5);
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenCalledTimes(5);
    await user.keyboard("{Home}");
    expect(onChange).toHaveBeenLastCalledWith(1);
    // Left from one star clears; so does Delete.
    await user.keyboard("{ArrowDown}");
    expect(onChange).toHaveBeenLastCalledWith(null);
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "1 star" }));
    await user.keyboard("{End}{Delete}");
    expect(onChange).toHaveBeenLastCalledWith(null);
    expect(checked()).toEqual([]);
  });

  it("does nothing while disabled", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: 2, onChange, disabled: true }));
    await user.click(screen.getByRole("radio", { name: "4 stars" }));
    expect(onChange).not.toHaveBeenCalled();
    expect((screen.getByRole("radio", { name: "4 stars" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });
});
