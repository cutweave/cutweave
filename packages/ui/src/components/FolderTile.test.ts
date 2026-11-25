// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { FolderTile } from "./FolderTile.tsx";
import { MediaBreadcrumb } from "./MediaBreadcrumb.tsx";

afterEach(cleanup);

describe("FolderTile", () => {
  it("is an option named for the folder and its count", () => {
    render(
      h(FolderTile, {
        name: "Interviews",
        countLabel: "3 items",
        selected: true,
        kindLabel: "Folder",
      }),
    );
    const tile = screen.getByRole("option", { name: "Folder, Interviews 3 items" });
    expect(tile.getAttribute("aria-selected")).toBe("true");
    expect(tile.getAttribute("tabindex")).toBe("-1");
  });

  it("only the active tile is in the tab order and a drop target says so", () => {
    render(h(FolderTile, { name: "A", countLabel: "Empty", active: true, dropTarget: true }));
    const tile = screen.getByRole("option");
    expect(tile.getAttribute("tabindex")).toBe("0");
    expect(tile.getAttribute("data-drop-target")).toBe("true");
  });

  it("is a group, not an option, while its name is edited", () => {
    render(h(FolderTile, { name: "X", countLabel: "Empty", renaming: true }));
    expect(screen.queryByRole("option")).toBeNull();
    expect(screen.getByRole("group")).toBeTruthy();
  });

  it("renames in place: Enter commits, Escape cancels", () => {
    const onRename = vi.fn();
    const onCancel = vi.fn();
    render(
      h(FolderTile, {
        name: "New folder",
        countLabel: "Empty",
        renaming: true,
        renameLabel: "Folder name",
        onRename,
        onCancelRename: onCancel,
      }),
    );
    const input = screen.getByRole("textbox", { name: "Folder name" }) as HTMLInputElement;
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: "Day 1" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onRename).toHaveBeenCalledWith("Day 1");
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("Escape cancels without committing on the blur that follows", () => {
    const onRename = vi.fn();
    const onCancel = vi.fn();
    render(
      h(FolderTile, {
        name: "X",
        countLabel: "Empty",
        renaming: true,
        onRename,
        onCancelRename: onCancel,
      }),
    );
    const input = screen.getByRole("textbox");
    fireEvent.keyDown(input, { key: "Escape" });
    fireEvent.blur(input);
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onRename).not.toHaveBeenCalled();
  });
});

describe("MediaBreadcrumb", () => {
  const segments = [
    { id: null, label: "Media" },
    { id: "fld_1", label: "Interviews" },
    { id: "fld_2", label: "Day one" },
  ];

  it("is a nav whose last segment is the current page", () => {
    render(h(MediaBreadcrumb, { label: "Media folders", segments, onNavigate: () => {} }));
    expect(screen.getByRole("navigation", { name: "Media folders" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Day one" }).getAttribute("aria-current")).toBe(
      "page",
    );
    expect(screen.getByRole("button", { name: "Media" }).getAttribute("aria-current")).toBeNull();
  });

  it("navigates to a segment and reports drops on it", () => {
    const onNavigate = vi.fn();
    const onDrop = vi.fn();
    render(
      h(MediaBreadcrumb, { label: "Media folders", segments, onNavigate, onSegmentDrop: onDrop }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Interviews" }));
    expect(onNavigate).toHaveBeenCalledWith("fld_1");
    fireEvent.click(screen.getByRole("button", { name: "Media" }));
    expect(onNavigate).toHaveBeenLastCalledWith(null);
    fireEvent.drop(screen.getByRole("button", { name: "Interviews" }));
    expect(onDrop.mock.calls[0]?.[0]).toEqual({ id: "fld_1", label: "Interviews" });
  });

  it("marks the segment a drag is over", () => {
    render(
      h(MediaBreadcrumb, {
        label: "Media folders",
        segments,
        dropTarget: null,
        onNavigate: () => {},
      }),
    );
    expect(screen.getByRole("button", { name: "Media" }).getAttribute("data-drop-target")).toBe(
      "true",
    );
    expect(
      screen.getByRole("button", { name: "Interviews" }).getAttribute("data-drop-target"),
    ).toBeNull();
  });
});
