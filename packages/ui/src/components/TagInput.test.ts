// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TagInput, type TagInputTag } from "./TagInput.tsx";

afterEach(cleanup);

const KNOWN = ["b-roll", "interview", "keep", "night"];

interface HarnessProps {
  initial?: TagInputTag[];
  onAdd?: (text: string) => void;
  onRemove?: (tag: string) => void;
  onPromote?: (tag: string) => void;
  error?: string;
  /** Leave the typed text in the field after an add (a host that refused it). */
  keepText?: boolean;
}

function Harness(props: HarnessProps) {
  const [tags, setTags] = useState<TagInputTag[]>(props.initial ?? []);
  const [value, setValue] = useState("");
  const typed = value.trim().toLowerCase();
  return h(TagInput, {
    tags,
    value,
    onValueChange: setValue,
    onAdd: (text: string) => {
      props.onAdd?.(text);
      if (props.keepText) return;
      setTags([...tags, { tag: text.trim() }]);
      setValue("");
    },
    onRemove: (tag: string) => {
      props.onRemove?.(tag);
      setTags(tags.filter((t) => t.tag !== tag));
    },
    onPromote: props.onPromote,
    suggestions: KNOWN.filter((t) => t.includes(typed) && !tags.some((x) => x.tag === t)),
    label: "Add tag",
    tagsLabel: "Tags",
    suggestionsLabel: "Suggested tags",
    removeLabel: (tag: string) => `Remove tag ${tag}`,
    promoteLabel: (tag: string) => `Add ${tag} to all`,
    error: props.error,
  });
}

const field = () => screen.getByRole("combobox", { name: "Add tag" }) as HTMLInputElement;
const chips = () =>
  screen.queryByRole("list", { name: "Tags" }) === null
    ? []
    : within(screen.getByRole("list", { name: "Tags" }))
        .getAllByRole("listitem")
        .map((li) => li.getAttribute("data-tag"));

describe("TagInput", () => {
  it("adds what is typed on Enter and on a comma, and never types the comma", async () => {
    const onAdd = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onAdd }));
    await user.type(field(), "sunset{Enter}");
    expect(onAdd).toHaveBeenLastCalledWith("sunset");
    expect(field().value).toBe("");
    await user.type(field(), "wide shot,");
    expect(onAdd).toHaveBeenLastCalledWith("wide shot");
    expect(field().value).toBe("");
    expect(chips()).toEqual(["sunset", "wide shot"]);
  });

  it("adds nothing for a blank field", async () => {
    const onAdd = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onAdd }));
    await user.type(field(), "  {Enter},");
    expect(onAdd).not.toHaveBeenCalled();
  });

  it("removes the last chip with Backspace in the empty field, not while there is text", async () => {
    const onRemove = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: [{ tag: "a" }, { tag: "b" }], onRemove }));
    await user.type(field(), "x{Backspace}");
    expect(onRemove).not.toHaveBeenCalled();
    await user.keyboard("{Backspace}");
    expect(onRemove).toHaveBeenCalledExactlyOnceWith("b");
    expect(chips()).toEqual(["a"]);
  });

  it("removes a chip with its named button", async () => {
    const onRemove = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { initial: [{ tag: "a" }, { tag: "b" }], onRemove }));
    await user.click(screen.getByRole("button", { name: "Remove tag a" }));
    expect(onRemove).toHaveBeenCalledExactlyOnceWith("a");
    expect(chips()).toEqual(["b"]);
  });

  it("marks a partial tag, shows its detail and offers to add it to all", async () => {
    const onPromote = vi.fn();
    const user = userEvent.setup();
    render(
      h(Harness, {
        initial: [{ tag: "all" }, { tag: "some", partial: true, detail: "2 of 3" }],
        onPromote,
      }),
    );
    const list = screen.getByRole("list", { name: "Tags" });
    const [all, some] = within(list).getAllByRole("listitem");
    expect(all?.getAttribute("data-partial")).toBeNull();
    expect(some?.getAttribute("data-partial")).toBe("true");
    expect(some?.textContent).toContain("2 of 3");
    expect(screen.queryByRole("button", { name: "Add all to all" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Add some to all" }));
    expect(onPromote).toHaveBeenCalledExactlyOnceWith("some");
  });

  it("lists suggestions while focused; Down, Up and Enter take one", async () => {
    const onAdd = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onAdd }));
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(field().getAttribute("aria-expanded")).toBe("false");
    await user.click(field());
    const list = screen.getByRole("listbox", { name: "Suggested tags" });
    expect(field().getAttribute("aria-expanded")).toBe("true");
    expect(field().getAttribute("aria-controls")).toBe(list.id);
    expect(within(list).getAllByRole("option")).toHaveLength(4);
    await user.keyboard("n");
    const names = () =>
      within(screen.getByRole("listbox"))
        .getAllByRole("option")
        .map((o) => o.textContent);
    expect(names()).toEqual(["interview", "night"]);
    await user.keyboard("{ArrowDown}{ArrowDown}");
    const active = field().getAttribute("aria-activedescendant");
    expect(active).not.toBeNull();
    expect(document.getElementById(active ?? "")?.textContent).toBe("night");
    expect(screen.getByRole("option", { name: "night" }).getAttribute("aria-selected")).toBe(
      "true",
    );
    await user.keyboard("{ArrowUp}{Enter}");
    expect(onAdd).toHaveBeenCalledExactlyOnceWith("interview");
    expect(chips()).toEqual(["interview"]);
  });

  it("takes a suggestion on click and keeps the focus in the field", async () => {
    const onAdd = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onAdd }));
    await user.click(field());
    await user.click(screen.getByRole("option", { name: "keep" }));
    expect(onAdd).toHaveBeenCalledExactlyOnceWith("keep");
    expect(document.activeElement).toBe(field());
  });

  it("puts the suggestions away on Escape without passing the key on, then lets it through", async () => {
    const outer = vi.fn();
    const user = userEvent.setup();
    render(h("div", { onKeyDown: (e: { key: string }) => outer(e.key) }, h(Harness, {})));
    await user.click(field());
    expect(screen.getByRole("listbox")).toBeTruthy();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(outer).not.toHaveBeenCalledWith("Escape");
    await user.keyboard("{Escape}");
    expect(outer).toHaveBeenCalledWith("Escape");
    // Typing brings them back.
    await user.keyboard("k");
    expect(screen.getByRole("listbox")).toBeTruthy();
  });

  it("does not pass Enter on to a dialog's primary action", async () => {
    const outer = vi.fn();
    const user = userEvent.setup();
    render(h("div", { onKeyDown: (e: { key: string }) => outer(e.key) }, h(Harness, {})));
    await user.type(field(), "x{Enter}");
    expect(outer).not.toHaveBeenCalledWith("Enter");
  });

  it("hands pasted text with commas over whole", () => {
    const onAdd = vi.fn();
    render(h(Harness, { onAdd, keepText: true }));
    fireEvent.paste(field(), { clipboardData: { getData: () => "one, two,three" } });
    expect(onAdd).toHaveBeenCalledExactlyOnceWith("one, two,three");
  });

  it("shows an error, marks the field invalid and describes it", () => {
    render(h(Harness, { error: "Use 64 characters or fewer." }));
    expect(field().getAttribute("aria-invalid")).toBe("true");
    const described = field().getAttribute("aria-describedby") ?? "";
    expect(document.getElementById(described)?.textContent).toBe("Use 64 characters or fewer.");
    expect(screen.getByRole("alert").textContent).toBe("Use 64 characters or fewer.");
  });
});
