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
| F1 | First launch | Start the app | Editor layout (assets, preview, inspector, timeline), "Engine connected", empty untitled project, no errors in the main log | planned |
| F2 | New project | File > New Project (Ctrl+N), name it, pick a folder | Title shows the name; the `.cutweave` folder exists on disk; no unsaved dot | planned |
| F3 | Import media | Import in the media bin (picker returns the demo/corpus files); switch Grid/List; scroll overflowing tabs | Cards show name, kind, duration, size and fps; the files are listed in the project | planned (probe via the dev stub until WP-1I.05) |
| F4 | Build a timeline | Drag clips from the bin to tracks; insert a clip with Enter; toggle snapping | Clips land where dropped; snapping aligns edges; tracks are created as needed | planned |
| F5 | Edit on the timeline | Select, move by dragging, trim by an edge, split with the razor and at the playhead, delete, ripple; zoom in and out | Positions and durations change as dragged; split makes two clips; delete removes; ripple closes the gap | planned |
| F6 | Inspector | Select a clip; edit name, start and duration (typed timecode), opacity, position, scale, rotation and gain (typing and scrubbing) | Fields show the new values; the timeline reflects timing changes; invalid input is rejected with a message | planned |
| F7 | Undo and redo | Ctrl+Z / Ctrl+Y (and the toolbar buttons) across F4–F6 | Each user action undoes and redoes as one step, in order; the inspector and timeline follow | planned |
| F8 | Save, close, reopen | Ctrl+S; quit; relaunch; open the project from Recent | The reopened timeline is identical; no unsaved dot | planned |
| F9 | Unsaved changes on quit | Edit, quit; answer Cancel, then Save; again with Discard | Cancel keeps the app and the edit; Save quits with the edit on disk; Discard quits without it | **covered** (`e2e/unsaved-quit.spec.ts`) |
| F10 | Unsaved changes on New/Open | Edit, then File > New or Open; answer Cancel / Discard / Save | Same three outcomes as F9, then the new or opened project | planned |
| F11 | Engine crash mid-edit | Edit; the engine is killed; keep editing | "Reconnecting" then back; every edit applied exactly once; nothing lost | covered in part (`e2e/resync.spec.ts`); planned as a user flow |
| F12 | App crash and recovery | Edit without saving; kill the whole app; relaunch | Recovery dialog; Restore brings the edits back, Discard doesn't | planned |
| F13 | Preview and transport | Space, J/K/L, arrows, step and go-to buttons, volume and mute, aspect switch | Timecode and preview frames advance and stop; the playhead follows | blocked by WP-1I.05 (real engine playback); runs today against the fake engine in `e2e/preview.spec.ts` |
| F14 | Export | File > Export (Ctrl+M), choose settings, export; cancel one | An MP4 that `ffprobe` accepts, with the timeline's duration and audio; cancel leaves no file | blocked by WP-1E.03, WP-1M.08, WP-1I.05 |
| F15 | An agent edits while the UI is open | A CLI or MCP client attached to the running engine adds and trims a clip | The UI shows the change live; Ctrl+Z in the UI undoes it | planned |
| F16 | Appearance and layout | Dark/Light, Short-form/Long-form, resize panels, relaunch | The choice applies at once and persists across a relaunch | planned |
| F17 | Real media | F3–F8 with real decode: thumbnails, waveforms, frames from H.264, HEVC 10-bit and VFR phone clips | Real thumbnails and waveforms, frame-accurate preview | blocked by WP-1D.03, WP-1D.05, WP-1D.06, WP-1I.05 |
