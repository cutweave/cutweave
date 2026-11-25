// SPDX-License-Identifier: GPL-3.0-or-later
import { InlineRename } from "./InlineRename.tsx";

export default { title: "Inputs/InlineRename", component: InlineRename, tags: ["autodocs"] };

const noop = () => {};

/** A name edited where it is shown; the text starts selected. Enter commits, Escape cancels. */
export const Editing = {
  render: () => (
    <div style={{ padding: "var(--space-4)", background: "var(--surface-1)", width: 240 }}>
      <InlineRename value="New folder" label="Folder name" onCommit={noop} onCancel={noop} />
    </div>
  ),
};
