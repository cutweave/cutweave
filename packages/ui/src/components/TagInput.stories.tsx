// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { TagInput, type TagInputTag } from "./TagInput.tsx";

export default { title: "Inputs/TagInput", component: TagInput, tags: ["autodocs"] };

const col = {
  padding: "var(--space-4)",
  display: "flex",
  flexDirection: "column",
  gap: "var(--space-3)",
  width: "var(--layer-width-md)",
  background: "var(--surface-1)",
} as const;

const KNOWN = ["b-roll", "interview", "keep", "night", "wide shot"];

interface FieldProps {
  initial: TagInputTag[];
  text?: string;
  error?: string;
  disabled?: boolean;
  autoFocus?: boolean;
}

function Field({ initial, text = "", error, disabled, autoFocus }: FieldProps) {
  const [tags, setTags] = useState(initial);
  const [value, setValue] = useState(text);
  const have = new Set(tags.filter((t) => !t.partial).map((t) => t.tag));
  const typed = value.trim().toLowerCase();
  return (
    <TagInput
      tags={tags}
      value={value}
      onValueChange={setValue}
      onAdd={(tag) => {
        setTags([...tags.filter((t) => t.tag !== tag.trim()), { tag: tag.trim() }]);
        setValue("");
      }}
      onRemove={(tag) => setTags(tags.filter((t) => t.tag !== tag))}
      onPromote={(tag) => setTags(tags.map((t) => (t.tag === tag ? { tag } : t)))}
      suggestions={KNOWN.filter((t) => !have.has(t) && t.includes(typed))}
      label="Add tag"
      tagsLabel="Tags"
      suggestionsLabel="Suggested tags"
      removeLabel={(tag) => `Remove tag ${tag}`}
      promoteLabel={(tag) => `Add ${tag} to all`}
      placeholder="Add tag"
      error={error}
      disabled={disabled}
      autoFocus={autoFocus}
    />
  );
}

/** No tags yet: the field alone, with its placeholder. */
export const Empty = {
  render: () => (
    <div style={col}>
      <Field initial={[]} />
    </div>
  ),
};

/** One asset's tags as removable chips. */
export const WithTags = {
  render: () => (
    <div style={col}>
      <Field initial={[{ tag: "b-roll" }, { tag: "interview" }, { tag: "wide shot" }]} />
    </div>
  ),
};

/**
 * Several assets selected: a tag all of them have is solid; one only some have is dashed, says
 * how many, and has a button that adds it to the rest.
 */
export const MixedSelection = {
  render: () => (
    <div style={col}>
      <Field
        initial={[
          { tag: "b-roll" },
          { tag: "interview", partial: true, detail: "2 of 3" },
          { tag: "night", partial: true, detail: "1 of 3" },
        ]}
      />
    </div>
  ),
};

/** Typing lists the project's tags that fit; Down and Enter take one. */
export const Suggestions = {
  render: () => (
    <div style={col}>
      <Field initial={[{ tag: "b-roll" }]} text="i" autoFocus />
    </div>
  ),
};

export const Invalid = {
  render: () => (
    <div style={col}>
      <Field
        initial={[{ tag: "b-roll" }]}
        text={"x".repeat(70)}
        error="Use 64 characters or fewer."
      />
    </div>
  ),
};

export const Disabled = {
  render: () => (
    <div style={col}>
      <Field initial={[{ tag: "b-roll" }, { tag: "interview" }]} disabled />
    </div>
  ),
};
