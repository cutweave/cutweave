# 10 - Detached panels: a panel in its own OS window

Date: 2026-11-25. Scope: how to let a person drag a panel out of the main window into its own window, the way Premiere Pro does, and dock it back; what it costs.
Trigger: product owner request, 2026-11-25.
Marks: [S] = shown by the spike (section 7); [U] = unverified (secondary source or not tested); [D] = recommendation.

## Summary

- **Recommendation [D]:** use dockview's own popout groups. The panel's DOM moves into a child window that Electron creates in the main window's renderer process, while React, the stores, the engine connection and the preview addon stay where they are. Nothing about the engine, the project format or the command surface changes.
- **The catch [S]:** Electron 45 isolates a popup whose sandbox setting differs from its opener's, so a sandboxed popout cannot be scripted. Popout windows must share the editor's (unsandboxed) process, with no preload. That bends the rule "every other window is sandboxed" (arch §2, §18), so it needs an ADR.
- **The work:** the spike (Windows) shows a detached Inspector updating live, a detached Preview showing frames, and detached windows restored after a relaunch, all without changes. Shortcuts and right-click menus do not work in a detached window yet. The editor has to become multi-window aware: shortcuts, menus, tooltips, size observers, animation frames and the theme must follow the window a panel is in.
- **Cost:** two work packages, about 6 agent-days: WP-2H.14 (popout windows and the shell, M) and WP-2H.15 (every panel works in its own window, M). One ADR (popout windows' process and security policy).
- **Not recommended:** a second renderer per window (option b). It costs about three times as much, and it needs a second preview path, a second engine connection and cross-window state sync.

## 1. Reference behaviour

| App | How a panel leaves | The window | Back into the dock | Saved | Shortcuts in it |
|---|---|---|---|---|---|
| **Premiere Pro** | Ctrl/Cmd-drag a panel or group out; panel menu "Undock Panel" / "Undock Panel Group" ([Adobe help, Premiere Elements, same panel system](https://helpx.adobe.com/premiere-elements/using/dock--undock--group-and-float-panels.html)) | Floating utility window; several panels can share one ([forum, 2026](https://community.adobe.com/questions-729/premiere-pro-windows-dual-monitor-fullscreen-workspace-limitation-1561906)) [U] | Drag its grip strip onto a drop zone; Window > Workspaces > Reset to Saved Layout docks everything ([forum](https://community.adobe.com/t5/premiere-pro-discussions/docking-a-floating-panel/td-p/5165424)) | In the workspace | Broke in v14.5 (shortcuts and drag and drop dead in undocked panels), fixed in 14.6, still reported later ([forum](https://community.adobe.com/t5/premiere-pro-discussions/bug-with-floating-panels-in-14-5/m-p/11524028), [beta forum](https://community.adobe.com/t5/premiere-pro-beta/keyboard-shortcuts-are-disabled-on-second-screen/m-p/11513115)) |
| **After Effects** | Same Adobe panel system [U] | | | | |
| **DaVinci Resolve** | No arbitrary undock. Workspace > Dual Screen moves the viewers and scopes to a second screen; scopes can float ([manual](https://www.steakunderwater.com/VFXPedia/__man/Resolve18-6/DaVinciResolve18_Manual_files/part4162.htm), [forum](https://forum.blackmagicdesign.com/viewtopic.php?p=415545)) | | | | |
| **Final Cut Pro** | No floating panels. Window > Show in Secondary Display > Viewer, Browser or Timeline ([Apple guide](https://support.apple.com/guide/final-cut-pro/verfa85dcd9e/mac)) | Full second display | One toolbar button returns all | | |
| **VS Code** (1.85+) | Drag an editor tab out of the window; "Move Editor into New Window" ([release notes](https://code.visualstudio.com/updates/v1_85)) | Real window, own title bar, "Always on top" toggle ([docs](https://code.visualstudio.com/docs/editor/custom-layout)) | Drag the tab back | Windows reopen where they were after a restart | Key listeners are registered on every window ([keybindingService.ts](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/services/keybinding/browser/keybindingService.ts)) |

VS Code is the closest technical model. Its auxiliary windows are `about:blank` popups opened from the main renderer ([auxiliaryWindowService.ts](https://github.com/microsoft/vscode/blob/main/src/vs/workbench/services/auxiliaryWindow/browser/auxiliaryWindowService.ts)). Its main process allows only those through `setWindowOpenHandler` ([app.ts](https://github.com/microsoft/vscode/blob/main/src/vs/code/electron-main/app.ts)). It copies and live-syncs the stylesheets into each popup, and listens for keys, focus and resizes per window.

**Behaviours to copy [D]:**

1. **Two ways out.** Drag a panel's tab outside the main window, which opens a window at the drop point (VS Code). Or choose **Move to new window** in the tab's menu, View > Panels or the palette. The in-window **Float** stays.
2. **A real OS window.** It has the OS title bar and controls, is titled with the panel name ("Inspector - MyVideo - Cutweave"), can go to any monitor, and can be maximized and made full screen. A **Keep on top** toggle is per window (VS Code). It is not an owned window: those hide from the taskbar on Windows and drag along with the main window on macOS.
3. **Several panels in one window.** Drop more tabs into a detached window (Premiere's groups; dockview's popouts hold a nested layout).
4. **Back into the dock.** Drag the tab back onto a drop zone in the main window, or choose **Move back to main window**. Closing the detached window also docks its panels back where they came from. Reset layout docks everything.
5. **Saved with the workspace.** Detached windows, their bounds and their panels belong to the workspace's layout, and a relaunch reopens them. A window whose monitor is gone reopens on a connected one. Premiere's handling of a disconnected monitor is undocumented [U].
6. **Every shortcut works in every window.** App shortcuts (Space, Ctrl+S, Ctrl+Z, the palette) work with a detached window focused. A panel's own keys follow that panel. This is the bug Premiere shipped in 14.5.
7. **One app.** Detached windows close with the main window. Dialogs open over the main window. The engine counts focus on a detached window as focus on its editor (agent targeting, ADR-0021).
8. **Later: preview on a second display.** A full-screen program monitor on a second display, like Final Cut and Resolve, mostly falls out of this (a detached Preview, full screen). A dedicated command can follow; it is not part of these WPs.

## 2. dockview popout groups

Our `dockview-react` 8.8.3 resolves `dockview-core` **8.9.1** (its dependency is `dockview: ^8.8.3`). Sources: the [docs](https://dockview.dev/docs/core/groups/popoutGroups) and the shipped source (`popoutWindow.ts`, `dockviewComponent.ts`; read in `node_modules`).

- **How it works.** `api.addPopoutGroup(panelOrGroup, {popoutUrl, position, onDidOpen, onWillClose})` calls `window.open(popoutUrl, "<dock>-<group>", "popup=yes,left,top,width,height")`. The default URL is `/popout.html`, and only same-origin URLs are accepted (custom schemes such as `app://` pass) ([PR #1647](https://github.com/dockview/dockview/pull/1647)). On the child's `load` it appends a container to the child's `<body>` and moves the group's DOM into it. React content keeps rendering from the main window's React root through dockview-react's portals, so components, Zustand stores and `window.cutweave` are the main window's.
- **Styles.** At load it copies the main document's stylesheets: `<link>` sheets as new links, inline and constructed sheets as one `<style>` per rule. It copies them **once**. It copies no `<html>` attributes, so `data-theme`, `data-density` and `lang` must be mirrored by us (VS Code also live-syncs `<head>`). Under our production CSP (`style-src 'self'`) the per-rule `<style>` copies are blocked. That is harmless while all our CSS is linked files, which it is in production builds.
- **Several groups per window.** A popout hosts a nested grid, so tabs can be dropped into it. Dragging a popout's group back into the main grid is supported (fixed in [#913](https://github.com/dockview/dockview/issues/913), [#960](https://github.com/dockview/dockview/issues/960), [#1004](https://github.com/dockview/dockview/issues/1004)) [U in Electron]. Mouse drags use HTML5 drag and drop (`dndStrategy: 'auto'`), which Chromium delivers across its own windows.
- **No drag-out gesture.** Dockview never opens a popout from a drag. We detect a tab drag that ends outside every window (`dragend` with no drop, its `screenX/screenY` outside the window) and call `addPopoutGroup` there [D].
- **Closing.** Closing the window puts the group back next to its reference group, or somewhere new. The main window's `beforeunload` closes all popouts, and a 250 ms poll of `closed` is a backstop. `group.api.moveTo(...)` docks back from code.
- **Layout.** `toJSON()` adds `popoutGroups: [{data | grid, url, gridReferenceGroup, position: {left, top, width, height}}]` with the position in screen pixels. `fromJSON()` reopens them 100 ms apart. A popup that fails fires `onDidOpenPopoutWindowFail` (`blocked`, `url-refused`, `unscriptable`, `closed`) and its panels stay in the grid ([#896](https://github.com/dockview/dockview/issues/896)). Our saved-layout code (`layoutMeta.ts`) already passes the field through.
- **Events and handles.** `onDidAddPopoutGroup`, `onDidRemovePopoutGroup`, `onDidPopoutGroupSizeChange`, `onDidPopoutGroupPositionChange`, `onWillClosePopoutWindow`, `getPopouts()` (live `Window` handles), `panel.api.getWindow()`.
- **Known issues worth knowing:**
  - [#851](https://github.com/dockview/dockview/issues/851): duplicate popouts when `onReady` runs twice (React StrictMode).
  - [#1477](https://github.com/dockview/dockview/issues/1477) and [#1532](https://github.com/dockview/dockview/issues/1532): listeners bound to the global `document`; fixed in 7.0.4. The same class of bug is ours to avoid (section 4).
  - [#1708](https://github.com/dockview/dockview/issues/1708) and [#1759](https://github.com/dockview/dockview/issues/1759): fixed by 8.6.2.
  - [#815](https://github.com/dockview/dockview/issues/815) and [#900](https://github.com/dockview/dockview/issues/900), open: restoring a popout to a floating group or exactly to its origin.
  - [#1291](https://github.com/dockview/dockview/issues/1291), open: popouts in their own process, which dockview does not do.
  - Dockview's own code notes that a `ResizeObserver` from the main window "fires unreliably across the window boundary", so it creates one in the popout's realm (`PopoutWindowService.observeGridviewSize`).

## 3. Electron

- **Same process, scriptable.** A same-site `window.open` child is created in the opener's renderer process, and the opener can reach its document directly ([window-open docs](https://www.electronjs.org/docs/latest/api/window-open)). That is what dockview needs.
- **The sandbox rule (Electron 45).** [electron#52472](https://github.com/electron/electron/pull/52472) (labelled 45-x-y; we ship 45.4.1) changed two things:
  - A child whose sandbox state differs from its opener's process is created in its own process with no opener: `window.open` returns `null`, and child windows default to sandboxed.
  - Our editor window is `sandbox: false` because its preload loads the frame-bridge addon (ADR-0001, transport (a)). So a policy-compliant sandboxed popout cannot work, and the popout must be `sandbox: false` with **no preload**. The spike confirms both halves [S]. A sandboxed popout landed in another process, dockview reported it `blocked`, the panel stayed docked, and an empty window was left open.
- **Why that is acceptable [D].**
  - The popout runs no code of its own: a blank page with no scripts, no preload, the app CSP and no navigation.
  - It is the same origin, process and JavaScript agent as the editor, so it adds nothing an attacker could reach that the editor window does not already expose.
  - The addon stays loaded once, in the editor's preload. The P1 check "a second window has no access to the addon" still holds for every other window.
  - When ADR-0001's target lands (GPU shared textures, a sandboxed editor), popouts become sandboxed with it.
- **Main-process wiring [D].**
  - `setWindowOpenHandler` allows exactly the editor window's `app://cutweave/popout.html` (dev: the Vite server's `/popout.html`), with a frame name matching dockview's `<dock>-<group>`. It returns `{sandbox: false}`, no preload, the native frame and the theme's background color, and keeps denying everything else as today (`hardening.ts`).
  - In `did-create-window`, main registers the window as a new kind `popout`, asserts its webPreferences (`window-policy.ts`), clamps its bounds to the connected displays (`fitToDisplays`, already used for the main window), and keeps its menu bar hidden.
- **Window state.** Bounds live in the dockview layout, in screen DIPs, so mixed-DPI monitors are fine. A monitor that is gone at launch is handled by the clamp. Wayland ignores programmatic positions ([platform notices](https://www.electronjs.org/docs/latest/api/browser-window#platform-notices)), so popouts open where the compositor puts them.
- **Owned or not.** `parent: mainWindow` keeps a child above its parent. On macOS it moves with the parent, and on Windows it leaves the taskbar ([parent and child windows](https://www.electronjs.org/docs/latest/api/browser-window#parent-and-child-windows)). Use independent windows plus a **Keep on top** toggle [D].
- **Focus.**
  - The main process tells the engine when "the window" gains focus (`mainWindow.on("focus")`, `project.report_focus`, ADR-0021). A popout's focus must count as its editor's, or an agent's default engine flips while a person works in a detached Inspector.
  - The macOS app menu already sends `menu:command` to the main window (`menu.ts`), so menu accelerators work from a focused popout.
- **Throttling [U].**
  - Chromium throttles `requestAnimationFrame` in a hidden or fully covered window. Several loops schedule on the **main** window's `requestAnimationFrame`: the preview's present loop (in the preload, `present-loop.ts`), the timeline, the JPEG preview and gestures.
  - So a detached Preview or Timeline may stall while the main window is minimized, unless each loop uses its own window's `requestAnimationFrame`.
  - The spike could not settle this. Playwright's Electron loader turns throttling off (`--disable-renderer-backgrounding`, `--disable-backgrounding-occluded-windows`), so the detached Preview kept presenting at 30 fps with the main window minimized, which proves nothing. It needs a manual check.

## 4. What changes in our app

| Area | Today | In a detached window | Fix [D] |
|---|---|---|---|
| Preview frames | The page hands its canvas to the preload (`attachCanvas`), which builds a WebGPU presenter on the main window's GPU device and presents on the main window's frames | Works on Windows [S]. The canvas moved with its panel, the GPU path stayed live, and frames changed on a seek and during playback (30 fps presented, no presenter errors). | Run the present loop on the canvas's own window (`canvas.ownerDocument.defaultView`). Check macOS and Linux, and re-attach the canvas there if a moved canvas loses its context |
| Preview size | `ResizeObserver` from the main realm and the global `devicePixelRatio` (`PreviewCanvas.tsx`) | Followed the popout's size on open and after a resize, 680 × 383 then 790 × 444 [S]. A monitor with another DPI is untested [U] | Observe with the element's own window's `ResizeObserver`, and read that window's `devicePixelRatio` |
| Timeline canvas | Already reads `devicePixelRatio` and `matchMedia` from its canvas's window, but uses the global `ResizeObserver` and `requestAnimationFrame` (`timeline-view.ts`) | Drawn at the popout's size, 1406 × 144 [S]. Repainting while the main window is hidden is untested [U] | Same per-window observer and frame scheduler |
| Shortcuts | Six `window.addEventListener("keydown")` sites (keymap dispatcher, transport, app commands, bin, settings, menu bar) and `document.activeElement` checks | Dead: Space pressed in a popout did nothing [S]. With a shim that re-sent the popout's keys to the main window, Space in the detached Timeline started playback [S] | A window registry (main plus each popout, `onDidAddWindow`) that every key listener uses; the "focused surface" checks read the focused window's active element (VS Code's pattern) |
| Context menus | `installContextMenuHandlers(document)` once; the `Menu` renders through Radix's portal into the main `document.body` | Dead: a right-click did nothing [S]. With the handlers installed on the popout's document, the right clip menu (Split at playhead, Delete clip, ...) opened, but in the main window at the popout's coordinates [S] | Install the handlers on each popout's document; a portal-container context per panel window for `Menu`, `Tooltip` and `Dialog` in `packages/ui` |
| Dialogs | Radix `Dialog` into the main body | A dialog raised from a popout opens in the main window [U] | Keep app dialogs in the main window and focus it [D] |
| Drag from the bin into a detached Timeline | HTML5 drag and drop plus `dragStore` (module state, same realm) | Should work: Chromium routes drags between its windows [U] | Cover it in the flow (needs real OS input, like the window-drag steps) |
| Theme, density, language | Attributes on the main `<html>` (`EditorShell.tsx`) | Tokens and fonts arrive with the copied stylesheet, and the dark theme looked identical [S]. The light theme needs `data-theme` on the popout's `<html>`, which dockview does not copy | Mirror `data-theme`, `data-density`, `lang` and `dir` into each popout and keep them in sync; future interface scale (WP-2H.09) too |
| `instanceof` checks | 13 sites use `x instanceof HTMLElement` and similar | React still creates the panel's nodes in the main window's realm, so most checks pass [S]. Events fired in the popout, and nodes a library creates with the popout's `ownerDocument`, belong to the popout's realm | Duck-type event and node checks (`nodeType`, `tagName`, `"key" in e`); a lint rule |
| i18n, a11y | One `i18next` instance | Same realm, so translations work. Dockview mirrors its live regions into popouts ([#1368](https://github.com/dockview/dockview/pull/1368)) | Title each window by its panel; run axe on a popout document |
| Engine focus (agents) | Main window focus only | A popout's focus reads as "app lost focus" | Count popout focus as the editor's |
| Reload | Dev reload or engine-port change reloads the renderer | `beforeunload` closes popouts; the layout restore reopens them | Nothing extra; covered by the relaunch check |
| User flows | One `page` per app | Playwright gives each popout its own `Page` (`electronApp.waitForEvent("window")`), and clicks and keys work per window [S] | A drag from one window to another needs real OS input, so that step is opt-in (`just e2e-real-input`) |

## 5. Options

| Option | How | Pros | Cons | Effort |
|---|---|---|---|---|
| **(a) dockview popout groups** [D] | Same-process child windows; panels' DOM moved in; one React root | One engine connection, one set of stores, one preview presenter; drag between windows comes from dockview; layout save is built in | Popouts share the editor's unsandboxed process (ADR); realm and "which window" bugs in our code (section 4); dockview's popout code is young | ~6 agent-days (two M WPs) |
| (b) A renderer per window | A second `BrowserWindow` loading the editor in "panel mode" with its own React root | Crash and sandbox isolation per window; no realm issues | Needs a second engine connection, and sync of UI state (selection, tools, zoom) across windows. The preview needs a second addon preload (against ADR-0001) or shared textures. No tab drag between windows (separate dockview instances; cross-instance drag is an enterprise feature, [PR #1704](https://github.com/dockview/dockview/pull/1704)). Two copies of the app in memory | L+ (~15 agent-days), higher risk |
| (c) Second-display modes only | Fixed windows: program monitor (and maybe the bin) full screen on a second display, like Resolve and Final Cut | Small, predictable | Not what was asked: no arbitrary panel | S-M; better built later on top of (a) |
| (d) In-window floating only | Today's `addFloatingGroup` | Done | Cannot leave the main window | 0 |

## 6. Risks

- **Electron tightens `window.open` again.** It just did in 45. Mitigation: a unit test of the handler and policy, plus the user flow that opens a popout. Both fail loudly on an Electron upgrade.
- **The security exception.** Popouts run unsandboxed (process-shared). Mitigation: the ADR; tests that a popout has no preload, no `cutweave` global of its own, cannot navigate or `window.open`, and serves the app CSP.
- **A long tail of "wrong window" bugs.** Global `document`, `window`, `ResizeObserver`, `requestAnimationFrame` and `instanceof` in panel code. Mitigation: the window registry and helpers (`windowOf(el)`, `activeDocument()`), and a lint rule banning those globals in `packages/editor`, `packages/ui` and `packages/timeline` outside an allowlist.
- **Dockview's popout code moves fast** (8.6 to 8.9 fixed several popout bugs). Mitigation: keep all calls in `dockHost.ts`, pin the version, and cover popouts in the flow.
- **Linux.** `dragend` coordinates are unreliable on some Linux setups [U], so the drag-out gesture may not place the window. The menu item always works. Wayland ignores saved positions.
- **Preview on a second GPU.** On hybrid laptops a monitor may be driven by the other GPU. The presenter's device comes from the main window's adapter, and Chromium's single GPU process composites both windows [U]. Measure with the playback probe once WP-2H.15 lands.

## 7. Spike

**Setup.** Windows 11, Electron 45.4.1, dockview-core 8.9.1, the real engine with FFmpeg and the corpus trailer on V1. The spike code was:
- `popout.html`, a blank page;
- a `setWindowOpenHandler` branch for it;
- `popoutPanel(id)` in `dockHost.ts`;
- shims for keys, menus and the theme;
- a Playwright script that drives each popout as its own `Page`.

None of it is in this branch. Results:

| Check | Result |
|---|---|
| Popout under today's policy (`sandbox: true`) | **Fails.** Electron put the window in another process, `window.open` returned `null`, and dockview reported `blocked`. The Inspector stayed docked, and an empty window was left open |
| Popout with `sandbox: false` and no preload | **Works.** Same process id as the editor, no preload (`getLastWebPreferences`), and no `cutweave` or `require` in the popout's own global |
| Renders with our styles | **Yes.** Dockview copied the one built stylesheet: tokens resolve (`--surface-1`), Instrument Sans loads, and the Inspector looks identical to the docked one |
| Inspector updates live | **Yes.** Selecting a clip in the main timeline shows it in the detached Inspector. Renaming it there reaches the engine (`item.get`) |
| Preview shows frames | **Yes,** on the GPU path. Frames change on a seek and during playback (30 fps presented, no presenter errors), and the canvas follows the window's size |
| A shortcut in the popout | **No** without work. Space did nothing. With a shim that forwards keys to the main window's listeners, Space in the detached Timeline played |
| A right-click menu in the popout | **No** without work. Nothing opened. With the menu handlers installed on the popout's document, the correct clip menu opened, but in the main window (Radix portal) |
| Layout restores after relaunch | **Yes.** `toJSON()` held three popouts with their screen bounds. After a relaunch, all three windows reopened at those bounds with their panels. Electron has no popup blocker in the way |
| Realm | Nodes React renders inside a popout are main-window objects, so most `instanceof` checks keep working |

Not covered:
- dragging a tab between windows, and the drag-out gesture (Playwright cannot drag across OS windows);
- macOS and Linux;
- a second monitor with another DPI;
- throttling with the main window minimized (Playwright disables throttling; section 3).

The spike's code was about 150 lines. Most of the time went to waiting for the shared GPU lock.

## 8. Proposed work packages

Two WPs in Epic 2H, about 6 agent-days. The first can merge alone: panels that need no keys or overlays (Inspector, History, Assets) work in it already. The second makes the Preview, the Timeline and every shortcut and menu work there.

### WP-2H.14 — Detached panels I: popout windows, the shell and the layout
- **Area:** apps/desktop/src/main (`window-policy.ts`, `windows.ts`, `hardening.ts`, `index.ts` focus wiring, a `window:setPopoutOnTop` channel in the IPC contract and `handlers.ts`), apps/desktop/src/preload (the one new bridge method), packages/editor/public/popout.html, packages/editor/src/workspace/ (`dockHost.ts`, `commands.ts`), packages/editor/src/dock/, packages/editor/src/shell/ (a window registry), packages/editor/src/menus/surfaces (the `dock.tab` items), the `en` catalog, apps/desktop/e2e (security, flows), docs/user-flows.md, docs/adr/ (the ADR, Proposed), docs/01-architecture.md §2, §16, §18
- **Depends on:** WP-2H.01, WP-2H.12, WP-2H.13 (the dock tab menu), the ADR in section 9
- **Size:** M
- **Executor:** opus (it changes the window security policy and the Electron process model)
- **Scope:**
  - **Main process:**
    - A `popout` window kind: the editor's process, `sandbox: false` (Electron 45 requires it to share the opener's process), no preload, the locked settings of every kind.
    - `setWindowOpenHandler` allows only an editor window's `popout.html` on the app origin (dev: the Vite server), with dockview's frame name (`<dock>-<group>`). Everything else is denied as today.
    - `did-create-window` registers the kind, asserts the policy, clamps the bounds to the connected displays (`fitToDisplays`), hides the menu bar and paints the theme's background.
    - A popout's focus counts as its editor's for `project.report_focus` (ADR-0021).
    - `window:setPopoutOnTop{frame, on}` (zod-validated) for Keep on top.
  - **Dock:**
    - `workspace.popout_panel{panel}` and `workspace.dock_panel{panel}` (UI commands, so the palette and keymap list them).
    - "Move to new window" and "Move back to main window" in the tab menu.
    - Dragging a tab outside the main window opens it in a new window at the pointer. A tab drag that ends with no drop outside the window counts as that.
    - Closing a detached window docks its panels back where they were.
    - A failed popout (`onDidOpenPopoutWindowFail`) leaves the panel docked, with a toast.
    - Reset layout and Reset all workspaces dock everything back. Switching workspaces closes the old workspace's windows and opens the new one's.
  - **Layout:**
    - `popoutGroups` are saved with each workspace and restored on launch. `layoutProblem` validates their panels like the grid's.
    - A layout saved by an older build still loads.
  - **Window registry** (`shell/windows.ts`):
    - Holds the main window and each popout, with `onDidAddWindow` and `onDidRemoveWindow`.
    - Mirrors `data-theme`, `data-density`, `lang` and `dir` into each popout and keeps them in sync.
    - Titles each popout "<Panel> - <Project> - Cutweave".
    - WP-2H.15 builds on it.
  - **Keep on top:** a toggle in the detached group's header.
- **Interfaces:** the `popout` window kind and its policy; `window:setPopoutOnTop`; `workspace.popout_panel`, `workspace.dock_panel`; `popoutPanel(id)`, `dockPanel(id)` in `dockHost.ts`; the window registry (`registeredWindows()`, `onDidAddWindow`, `onDidRemoveWindow`).
- **Acceptance criteria:**
  - Vitest:
    - The policy accepts a `popout` only as `sandbox: false` with no preload.
    - The handler allows only `popout.html` from an editor window with the frame-name pattern and denies everything else.
    - `dockHost` round-trips a layout with a popout and keeps a failed popout docked (jsdom fake window).
  - Security e2e (`security.spec.ts`): a popout has no `cutweave` global of its own and no `require`; its navigations and its own `window.open` are blocked; its document has the app CSP; the addon is loaded once.
  - **A new user flow, "Detached panels" (ADR-0018; its number is allocated at merge, since F44 is taken):**
    1. Right-click the Inspector's tab and choose Move to new window: a second window titled with the panel name shows the Inspector, and the Preview widens.
    2. Click a clip in the main timeline: the detached Inspector shows it. Change its opacity there: the engine has the new value.
    3. Switch to the light theme: the detached window follows.
    4. Close the detached window: the Inspector is docked where it was.
    5. Move it out again and relaunch: the window reopens with the Inspector.
    6. Reset layout: it docks back.
    7. `quitApp` leaves no window behind.
  - The engine's focus record does not change when the focus moves between the editor and its detached window.
- **Tests:** Vitest, security e2e, the user flow.
- **Out of scope:** keys, context menus, tooltips, the Preview and the Timeline inside a detached window (WP-2H.15); a full-screen program monitor on a second display; detaching plugin panels (P6).

### WP-2H.15 — Detached panels II: every panel works in its own window
- **Area:** packages/editor (keymap dispatch, transport keymap, app commands, bin keys, the menu host, the palette, `preview/`, the timeline panel, scrub and gesture schedulers), packages/ui (a portal-container provider for `Menu`, `Tooltip` and `Dialog`; `Tabs`), packages/timeline (`timeline-view.ts`), apps/desktop/src/preload (`present-loop.ts`, `api.ts`: present on the canvas's window, re-attach), a lint rule (biome or a script in `just check`), apps/desktop/e2e/flows, docs/user-flows.md
- **Depends on:** WP-2H.14, WP-2H.05
- **Size:** M
- **Executor:** sonnet
- **Scope:**
  - **Per-window helpers:** `windowOf(node)`, `documentOf(node)`, `activeDocument()` and `forEveryWindow(install)`, built on WP-2H.14's registry.
  - **Keys:** every key listener (the keymap dispatcher, the transport, the app commands, the bin) is installed on every registered window. "Focused surface" checks read the focused window's active element. The menu bar's Alt handling stays in the main window.
  - **Menus and tooltips:**
    - `installContextMenuHandlers` runs on each window's document.
    - `packages/ui` gets a portal-container context. Each panel sets it from `panel.api.getWindow()` and updates it when the panel changes window, so `Menu`, `Tooltip` and the command palette open in the window the person is in.
    - App dialogs stay in the main window, which takes the focus.
  - **Preview:**
    - The preload's present loop schedules on the canvas's own window. The preload API keeps its shape.
    - The canvas is re-attached when its panel changes window only on a platform where a moved canvas loses its context. Windows does not.
    - Size and `devicePixelRatio` come from that window, and `view.set_preview_size` follows a DPI change.
  - **Timeline:**
    - `timeline-view.ts`, the gesture scheduler and the scrubber use their element's window for `ResizeObserver` and `requestAnimationFrame`.
    - So do the bin, History, `Tabs` and the media preview.
  - **Realm-safe checks:** the `instanceof` sites that can see popout events or popout-created nodes become duck-typed. A lint rule bans the bare globals `document`, `window`, `ResizeObserver`, `requestAnimationFrame` and `devicePixelRatio` in panel code, outside an allowlist.
- **Interfaces:** the helpers above; `PortalContainerProvider` in `packages/ui`; no engine or IPC change.
- **Acceptance criteria:**
  - Vitest: a key pressed in a second (jsdom) window runs its binding; a right-click there opens the menu in that window's document; the helpers return the element's own window; the lint rule flags a fixture.
  - **The "Detached panels" flow, extended (ADR-0018):**
    1. Detach the Timeline and the Preview.
    2. Press Space in the detached Timeline: the timecode runs, and the detached Preview's frames change.
    3. Right-click a clip in the detached Timeline: the menu opens in that window; choose Split at playhead. Press Ctrl+Z there: the split is undone.
    4. Press Ctrl+K there: the palette opens in that window.
  - Vitest: the present loop, the timeline and the gesture scheduler take their frames from their canvas's window.
  - A manual check on Windows, macOS and Linux: a detached Preview keeps playing while the main window is minimized. The flow cannot show it, because Playwright turns throttling off.
  - **Opt-in real-input steps** (`just e2e-real-input`): drag a bin card from the main window onto the detached Timeline (the clip lands at the drop point); drag the detached Timeline's tab back into the main dock.
  - With the Preview detached, playback drops no more frames than docked (the playback probe, under 2 per 10 s).
  - axe finds 0 violations in a detached window.
- **Tests:** Vitest, the user flow, the playback probe, axe.
- **Out of scope:** the second-display program monitor; per-window keymaps; detached plugin panels (P6).

## 9. ADR needed

- **ADR-00NN (Proposed): Detached panels: dockview popout windows in the editor's renderer process.** It records:
  - a third window kind, `popout`: the editor's process, `sandbox: false` because Electron 45 requires it, no preload, the app CSP, opened only by an editor window for `popout.html`;
  - the amended rule in arch §2 and §18 and in ADR-0001's consequences;
  - that detached windows are part of the workspace layout (user configuration, not the project format: no schema change);
  - that it follows ADR-0001 to a sandboxed renderer when shared textures land.

  The IPC contract gains one allowlisted channel and one bridge method (Keep on top). The preview bridge keeps its shape.
