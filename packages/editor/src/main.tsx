// SPDX-License-Identifier: GPL-3.0-or-later
import "@cutweave/ui/tokens/tokens.css";
import "@cutweave/ui/fonts/fonts.css";
import "dockview-react/dist/styles/dockview.css";
import "./app.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { EditorShell } from "./shell/EditorShell.tsx";

const root = document.getElementById("root");
if (!root) throw new Error("index.html is missing #root");
createRoot(root).render(
  <StrictMode>
    <EditorShell />
  </StrictMode>,
);
