# ADR-0031: Detached panels are dockview popout windows in the editor's renderer process

- **Status:** Accepted (product owner, 2026-11-26)
- **Date:** 2026-11-26
- **Deciders:** product owner, on the research in [10-detached-panels.md](../research/10-detached-panels.md)
- **Supersedes:** none (amends the window rule of arch §2 and §18 and the consequences of ADR-0001)
- **Architecture refs:** `docs/01-architecture.md` §2, §10, §16, §18

## Context

The product owner wants to drag a panel out of the main window into its own OS window, as Premiere Pro and VS Code allow, and dock it back. dockview, which lays out our panels, has popout groups: the panel's DOM moves into a child window opened with `window.open`, while React, the stores, the engine connection and the preview addon stay in the main renderer. The spike in research 10 showed a detached Inspector updating live, a detached Preview showing frames, and detached windows reopening after a relaunch.

Electron 45 isolates a popup whose sandbox setting differs from its opener's (electron#52472), so a sandboxed child cannot be scripted by the unsandboxed editor window. Arch §2 and §18 say every window except the main editor window is sandboxed.

## Considered options

1. **dockview popout windows in the editor's process, unsandboxed, no preload** (chosen). Cheapest (about 6 agent-days, WP-2H.14 and WP-2H.15); everything keeps working because it is the same JS context.
2. **A second renderer per window with its own engine connection and preview path.** About three times the cost, plus cross-window state sync. Rejected.
3. **Wait for GPU shared textures**, when every window can be sandboxed. Rejected for now: no date, and the owner wants the feature in Phase 2.

## Decision

We add a third window kind, **popout**: a window that only an editor window can open, for `popout.html`, through an allowlisted `setWindowOpenHandler` rule. It runs in the editor's renderer process with `sandbox: false` (Electron 45 requires it to match its opener), **no preload**, no Node integration, `contextIsolation` on and the app's CSP. It has no API of its own: all its code is the editor's, rendered through portals. Every other window stays sandboxed with no preload.

Detached windows, their bounds and their panels are part of the workspace layout (user configuration), not the project format: no schema change. The IPC contract gains one allowlisted channel and one bridge method (Keep on top); the preview bridge keeps its shape.

When the preview moves to GPU shared textures (ADR-0001's target), the editor and its popouts move to a sandboxed renderer together.

## Consequences

- WP-2H.14 (popout windows, shell and layout) and WP-2H.15 (shortcuts, menus, tooltips, Preview and Timeline in a detached window) are unblocked.
- The rule in arch §2 and §18 reads: every window is sandboxed except the main editor window and the popouts it opens, which share its process and have no preload.
- Security review: a popout can do nothing the editor window cannot; the CSP and the navigation guard apply to it as to the editor.
