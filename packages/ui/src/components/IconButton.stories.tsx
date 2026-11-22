// SPDX-License-Identifier: GPL-3.0-or-later
import { useState } from "react";
import { IconButton, Toolbar, ToolbarSeparator } from "./IconButton.tsx";

export default { title: "Actions/IconButton", component: IconButton, tags: ["autodocs"] };

const pad = { padding: "var(--space-4)", display: "flex", gap: "var(--space-3)" };

function ToolbarDemo() {
  const [tool, setTool] = useState("select");
  const [snap, setSnap] = useState(true);
  return (
    <div style={pad}>
      <Toolbar aria-label="Editing tools">
        <IconButton
          icon="mouse-pointer-2"
          label="Select (V)"
          pressed={tool === "select"}
          onClick={() => setTool("select")}
        />
        <IconButton
          icon="razor"
          label="Razor (C)"
          pressed={tool === "razor"}
          onClick={() => setTool("razor")}
        />
        <ToolbarSeparator />
        <IconButton
          icon="magnet"
          label="Snapping (S)"
          pressed={snap}
          onClick={() => setSnap(!snap)}
        />
        <IconButton icon="undo-2" label="Undo (Ctrl+Z)" disabled />
      </Toolbar>
    </div>
  );
}

export const InToolbar = { render: () => <ToolbarDemo /> };

export const Small = {
  render: () => (
    <div style={pad}>
      <IconButton size="sm" icon="lock" label="Lock track" />
      <IconButton size="sm" icon="eye" label="Hide track" pressed />
      <IconButton size="sm" icon="x" label="Close" disabled />
    </div>
  ),
};
