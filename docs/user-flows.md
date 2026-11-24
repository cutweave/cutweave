# User flows (the e2e regression suite)

These are the journeys a person takes through Cutweave. Each one runs end to end as a Playwright-Electron test in `apps/desktop/e2e/flows/`, against the built app, the real engine, real files and real media. The suite runs with `just e2e-flows` after every merge that touches app code (ADR-0018).

How a flow is written:

- **Drive it like a user.** Click buttons and menus, drag with the pointer, type into fields and press the shortcuts. Answer the app's own dialogs (Save / Discard / Cancel, crash recovery, naming) through the UI.
- **Stub only the OS.** Native file pickers return the path a user would pick, and external links are recorded. Read-only test hooks may observe state; nothing may change app behaviour.
- **Assert what the user sees and gets.** Visible text and roles, the timeline, the inspector, the top bar's unsaved dot, the saved project on disk, and exported files checked with `ffprobe`.
- **Wait for visible state, never sleep.** A flaky flow is a bug.
- **Teardown quits like a user** (`quitApp`). Every WP that adds or changes user-visible behaviour adds or extends a flow here and in the suite.

Status: **covered** (in the suite), **planned** (possible today, not written yet), or **blocked by WP-x** (needs that WP; that WP's acceptance includes un-blocking it).

| # | Flow | Steps | Expected | Status |
|---|---|---|---|---|
| F1 | First launch | Start the app | Editor layout (assets, preview, inspector, timeline), "Engine connected", empty untitled project, no errors in the main log | **covered** (`f01-first-launch.spec.ts`) |
| F2 | New project | File > New Project (Ctrl+N), name it, pick a folder | Title shows the name; the `.cutweave` folder exists on disk; no unsaved dot | **covered** (`f02-new-project.spec.ts`; also the name checks and Cancel) |
| F3 | Import media | Import in the media bin (picker returns the demo/corpus files); switch Grid/List; scroll overflowing tabs | Cards show name, kind, duration, size and fps; the files are listed in the project | **covered** (`f03-import-media.spec.ts`; also Ctrl+I, duplicates, a cancelled picker). Probes via the dev stub until WP-1I.05 |
| F4 | Build a timeline | Drag clips from the bin to tracks; insert a clip with Enter; toggle snapping | Clips land where dropped; snapping aligns edges; tracks are created as needed | **covered** (`f04-build-timeline.spec.ts`) |
| F5 | Edit on the timeline | Select, move by dragging, trim by an edge, split with the razor and at the playhead, delete, ripple; zoom in and out | Positions and durations change as dragged; split makes two clips; delete removes; ripple closes the gap | **covered** for select, move, trims, split at the playhead (S and the button), Delete/Backspace and zoom (+/−, Ctrl+wheel) (`f05-edit-timeline.spec.ts`). The razor tool and the zoom buttons are blocked by WP-2A.10; ripple by WP-2A.03 and WP-2A.04 |
| F6 | Inspector | Select a clip; edit name, start and duration (typed timecode), opacity, position, scale, rotation and gain (typing and scrubbing) | Fields show the new values; the timeline reflects timing changes; invalid input is rejected with a message | **covered** (`f06-inspector.spec.ts`) |
| F7 | Undo and redo | Ctrl+Z / Ctrl+Y (and the toolbar buttons) across F4–F6 | Each user action undoes and redoes as one step, in order; the inspector and timeline follow | **covered** (`f07-undo-redo.spec.ts`, with the Edit menu too). The first Ctrl+Z after typing in an inspector field now undoes the edit on Windows and Linux (the menu bar's shortcut, WP-1M.09); that step stays `fixme` on macOS, where the native menu still loses the key |
| F8 | Save, close, reopen | Ctrl+S; quit; relaunch; open the project from Recent | The reopened timeline is identical; no unsaved dot | **covered** (`f08-save-reopen.spec.ts`) |
| F9 | Unsaved changes on quit | Edit, quit; answer Cancel, then Save; again with Discard | Cancel keeps the app and the edit; Save quits with the edit on disk; Discard quits without it | **covered** (`f09-unsaved-quit.spec.ts`) |
| F10 | Unsaved changes on New/Open | Edit, then File > New or Open; answer Cancel / Discard / Save | Same three outcomes as F9, then the new or opened project | **covered** (`f10-unsaved-new-open.spec.ts`; also the recovery offer after Discard) |
| F11 | Engine crash mid-edit | Edit; the engine is killed; keep editing | "Reconnecting" then back; every edit applied exactly once; nothing lost | **covered** (`f11-engine-crash.spec.ts`; the protocol side call by call in `e2e/resync.spec.ts`) |
| F12 | App crash and recovery | Edit without saving; kill the whole app; relaunch | Recovery dialog; Restore brings the edits back, Discard doesn't | **covered** (`f12-app-crash-recovery.spec.ts`) |
| F13 | Preview and transport | Space, J/K/L, arrows, step and go-to buttons, volume and mute, aspect switch | Timecode and preview frames advance and stop; the playhead follows | blocked by WP-1I.05 (real engine playback); runs today against the fake engine in `e2e/preview.spec.ts` |
| F14 | Export | Import three corpus clips, place, trim and split them; File > Export (menu, Ctrl+M or the top bar's button); choose the file with the save dialog, change size and quality, hide and bring back the progress, cancel one; export a 1080p MP4; ask to replace it | The dialog starts at the sequence's size and rate and names the detected encoder; Export is off on an empty timeline; cancel leaves no file; the MP4 passes `ffprobe` (H.264 + AAC, 1920×1080, the timeline's duration within one frame) and plays in a Chromium `<video>` (`readyState >= 3`, `videoWidth` 1920); the toast reveals it in the folder; an existing file asks before it is replaced | written (`f14-export.spec.ts`, WP-1M.08), skips until the engine reads and exports real media: blocked by WP-1I.05 for the engine build (FFmpeg, real media wiring). The dialog, progress, chip, cancel, failure and replace prompt run today against the fake engine in `e2e/export-dialog.spec.ts` |
| F15 | An agent edits while the UI is open | A CLI or MCP client attached to the running engine adds and trims a clip; it opens another saved project | The UI shows the change live; it switches to the opened project (title, media, inspector); Ctrl+Z in the UI undoes the agent's edit | **covered** for the live update (`f15-agent-cli.spec.ts`, `cutweave exec` attached through the run-file). Ctrl+Z is `fixme`: the UI's undo is per actor (arch §5.2) and only undoes the person's own changes; needs a product decision |
| F16 | Appearance and layout | Dark/Light, Short-form/Long-form, resize panels, relaunch | The choice applies at once and persists across a relaunch | **covered** (`f16-appearance-layout.spec.ts`) |
| F17 | Real media | F3–F8 with real decode: thumbnails, waveforms, frames from H.264, HEVC 10-bit and VFR phone clips | Real thumbnails and waveforms, frame-accurate preview | blocked by WP-1D.03, WP-1D.05, WP-1D.06, WP-1I.05 |
| F18 | Window and menu (WP-1M.09) | Click every item of File, Edit, View and Help in the title bar's menu bar and press its shortcut (Ctrl+N/O/S/Shift+S/M, Ctrl+Z/Y/Shift+Z, Ctrl+X/C/V/A in a field, F11, dev: Ctrl+R, Ctrl+Shift+I); Alt, F10, arrows and Alt+letter; File > Exit with unsaved changes; drag and double-click the title area; switch the theme | Projects created, opened (also from Open recent), saved and saved as on disk; Export listed and disabled until WP-1M.08; undo/redo follow; Cut/Copy/Paste/Select all act on the field that had the focus and the clipboard; full screen, reload, developer tools and About work; Exit asks Save / Discard / Cancel and quits; no native menu; the OS window controls sit in a reserved overlay in the theme's colors; the window moves and maximizes/restores | **covered** (`e2e/flows/window-and-menu.spec.ts`; the OS-input steps run on Windows) |

## Running the suite

- `just e2e-flows` builds the engine (`cutweave-engine-serve`), the CLI and the app, then runs `apps/desktop/e2e/flows`. Extra arguments go to Playwright: `just e2e-flows -g F8` runs one flow.
- The engine runs with `--dev-stub-media` until WP-1I.05, so media is probed by extension and never decoded. The demo media is copied from `CUTWEAVE_DEMO_MEDIA` (default `E:/projects/cutweave-demo/media`); on a machine without it, small placeholder files of the same names stand in.
- Every run ends with a check that no Electron, engine or CLI process of this checkout is still running (`e2e/run-check.ts`). Leftovers are killed and fail the run.

## Writing a flow

Use the helpers in `apps/desktop/e2e/flows/helpers.ts`:

- `launchFlowApp` starts the app on the real engine; pass a profile from `profileDir()` to relaunch on the same settings and recent list.
- `chooseMenu(page, app, "File", "New project…")` is the only way a flow opens a menu: it clicks the title bar's menu bar on Windows and Linux (WP-1M.09) and runs the native menu's item on macOS. Pass a pattern for items named after the step (`/^Undo/`). `shortcut(app, "Control+S")` presses a menu shortcut.
- `pickFiles` answers the next OS file pickers. It is the only stub.
- `dropCard`, `drag`, `clickClip`, `timelineGeometry`, `xAt` and `laneY` find things on the timeline canvas from the same tokens and view state the renderer uses.
- `sequence`, `layout`, `item` and `mediaNames` read the document back from the engine. Use `editorCall` only to read state or for setup a flow does not test, never for the action under test.
