# Settings catalog

Every setting a person can change in Cutweave, with its default, where it lives, who may read or change it, and the work package that ships it. Planned 2026-11-24 at the product owner's request ("everything that can be customizable, will be"); the frame is WP-1M.11 ([P1](../wbs/P1-skeleton.md)), and each later page or row is built by the WP of its feature. When a WP adds, changes or drops a setting, it updates this file in the same PR; a test in WP-1M.11 checks that every registered key has a row here.

Benchmarks for coverage: the preferences of CapCut desktop, Premiere Pro, DaVinci Resolve, Final Cut Pro and Filmora (General, Appearance, Editing defaults, Timeline, Playback, Audio devices, Import and proxies, Export, Auto save, Shortcuts, Cache, Hardware, Updates). Cutweave adds Agents and MCP, AI providers, Copilot, Plugins and Privacy.

## Where settings live

| Scope | What it covers | Stored in | Changed through | Agents |
|---|---|---|---|---|
| **App** (preferences) | The person's choices, the same for every project | `preferences.json` in the app's user-data folder, owned by Electron main | The Settings dialog (File > Settings…, Ctrl+,; macOS: Cutweave > Settings…, ⌘,) | No access, except **read** for values the app applies to its engine |
| **Machine** (this computer) | Cache folder and limits, hardware, audio devices, local model storage | The same file, in its `machine` group; never exported or imported | The Settings dialog's "This computer" pages | **Read** (engine queries) |
| **Project** | What belongs to the edit: formats, still duration, proxy policy, budgets | The document, `project.settings` in `<Name>.cutweave` | File > Project settings… (WP-2H.08) and undoable commands | **Read and change**, like any edit (budget caps: read only) |
| **Secrets** | AI and service keys | The OS keychain, through write-only `ai.set_key` (arch §13) | AI providers page | Never readable by anyone; the UI shows fingerprints |

Rules:

- **Defaults for new projects** are app preferences that `project.new` copies into `project.settings`. After that the project's value rules, and File > Project settings… has "Use as defaults for new projects".
- **Preferences that change engine behaviour** are applied by the app to its own engine: at spawn as a flag (allowed folders, cache folder), or after each handshake and on change through an engine command in a **UI-only scope** (ADR-0016). Headless engines take flags, environment variables and project settings instead. Agents read the values in effect through `project.info` and `media.cache_status`; they never change preferences.
- **Not settings:** view state (playhead, zoom, selection, workspace layouts, the bin's layout, sort and open folder) stays per project in `.cutweave/ui-state.json` or in the editor's local storage; per-sequence settings are in the sequence dialog (WP-2B.03); clip properties are in the inspector.
- **Every row** has a label, a one-line description and search keywords; it can be reset on its own, with its page, or with everything (Advanced). Export and import cover App rows only.

### Adding a setting (for the WP that ships it)

1. Add the key to `packages/preferences/src/schema.ts` (`PreferenceValues`, `PREFERENCES`: type, scope, default, range). Main and the editor both read it, so a value the dialog cannot produce is refused by main as well. A change to a key's meaning or shape adds a migration there and bumps `SCHEMA_VERSION`.
2. Register the row with `registerSettingRow(pageId, row)` (or the whole page with `registerSettingsPage`) in `packages/editor/src/settings/`: key, label, one-line description, keywords, control (switch, segmented, select, number with unit, folder, list, action or custom) and who applies it (`now`, `engine` or `restart`). The strings go in `packages/editor/src/i18n/locales/en.json`.
3. Add its row to the tables below in the same change. Two tests fail otherwise: every registered key must have a row here, and every key in the schema must have a registered row.
4. Whoever applies the value reads it from the preferences store (`usePref` in the editor, `PreferencesStore` in main) or, for the engine, sends it after each handshake and on change (see `saving.*` in `engine/runtime.ts`).

## Pages

The dialog is a side tab list of these pages, in this order, with one search field across all of them. A page appears once one of its rows has shipped.

| # | Page | Group | First planned in |
|---|---|---|---|
| 1 | General | Preferences | WP-1M.11 |
| 2 | Appearance | Preferences | WP-1M.11 |
| 3 | Editing | Preferences | WP-2A.13 |
| 4 | Timeline | Preferences | WP-2A.13 |
| 5 | Playback | Preferences | WP-2H.10 |
| 6 | Audio | Preferences | WP-2E.08 |
| 7 | Media and import | Preferences | WP-1M.14 (hover-scrub), WP-2F.03 |
| 8 | Export | Preferences | WP-1M.13 |
| 9 | Saving and backups | Preferences | WP-1M.11 (rows from WP-1M.10) |
| 10 | Keyboard shortcuts | Preferences | WP-1M.13 (read-only), WP-2H.05 (editor) |
| 11 | Notifications | Preferences | WP-1J.05 |
| 12 | Agents and MCP | Agents and AI | WP-1J.05 |
| 13 | AI providers | Agents and AI | WP-3D.06 |
| 14 | Copilot | Agents and AI | WP-6I.08 |
| 15 | Plugins | Extensions | WP-6C.05 |
| 16 | Privacy | Privacy and updates | WP-3I.05 |
| 17 | Updates | Privacy and updates | WP-7A.06 |
| 18 | Cache and storage | This computer | WP-1D.08 |
| 19 | Performance and hardware | This computer | WP-1D.08 |
| 20 | Advanced | | WP-1M.11 |

## Settings by page

"Applied by" says who acts on the value: the renderer (editor UI), main (Electron), the engine (through a UI-only command, or a flag at start), or a project command.

### General

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `general.startup` | On startup: open the last project, start with an empty project, or show the welcome screen | Empty project (welcome screen once it exists) | App | Renderer | — | WP-1M.11 (welcome option: WP-3I.02) |
| `general.new_project_folder` | Default folder for new projects | `Documents/Cutweave` | App | Renderer (New project dialog) | — | WP-1M.11 |
| `general.recent_count` | Recent projects to list, and "Clear recent projects" | 10 | App | Main | — | WP-1M.11 |
| `general.dialog_choices` | Reset "Don't ask again" choices | None stored | App | Renderer | — | WP-1M.11 |
| `general.workspace` | Workspace for new projects: last used, Long-form or Short-form | Last used | App | Renderer | — | WP-2H.01 |
| `general.language` | Language | The OS language when Cutweave has it, else English | App | Renderer and main | — | WP-7C.04 |

### Appearance

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `appearance.theme` | Theme: Dark, Light, Match system | Dark | App | Main (window colors) and renderer | — | WP-1M.11 |
| `appearance.theme` (option) | High contrast theme (7:1) | — | App | Renderer | — | WP-7C.02 |
| `appearance.scale` | Interface scale: 90 to 150% | 100% | App | Main (zoom factor) | — | WP-2H.09 |
| `appearance.density` | Density: Standard or Comfortable | Standard | App | Renderer (tokens) | — | WP-2H.09 |
| `appearance.reduce_motion` | Reduce motion: follow the system, on, off | Follow the system | App | Renderer | — | WP-2H.09 |
| — | Accent color | Not offered: marigold is the one "here" color ([design/README.md](../../design/README.md)); pending the product owner's call | — | — | — | — |

### Editing

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `editing.still_duration` | Still image duration for new projects (freeze frames default to 3 s, set per freeze with `clip.freeze_frame`'s `duration`) | 5 s | App, copied to the project | `project.new` (`still_duration`) | Read the project's value (`project.info`: `settings.still_duration`) and change it (`project.set_settings`) | WP-2F.05b ([ADR-0035](../adr/0035-still-duration-and-image-sequences-in-the-project.md)) |
| `editing.sequence_format` | Format of new projects: the frame size, frame rate, audio sample rate, color space and background their sequences start with. Set by "Use as defaults for new projects" in File > Project settings… (the project's `default_format`); the Settings row shows it and puts Cutweave's own back. Stored as one line, `1080x1920 30000/1001 44100 rec709 #000000` | Not set (the engine's format: 1920 × 1080, 30 fps, 48 kHz) | App, copied to the project | `project.new` (`settings.default_format`) | Read the project's value (`project.info`: `settings.default_format`) and change it (`project.set_settings`) | #221 (split from WP-2H.08) |
| `editing.text_duration` | Duration of new text | 5 s | App | Renderer | — | WP-3A.07 |
| `editing.transition_duration` | Duration of new transitions | 0.5 s | App | Renderer | — | WP-4A.09 |
| `editing.audio_fade` | Length of a fade added with one click | 0.5 s | App | Renderer | — | WP-2E.05 |
| `editing.caption_lines` | New captions: characters per line and lines per caption | 32, 2 | App | Renderer | — | WP-3E.11 |
| `editing.caption_style` | Caption style for new captions | Cutweave default style | App | Renderer | — | WP-3E.11 |
| `editing.link_av` | Keep a clip's video and audio linked when inserted | On | App | Renderer (`timeline.insert`'s `link`) | — | WP-2A.13 |
| `editing.drop_mode` | Dropping media onto a clip (outside the main track): overwrite or insert | Overwrite | App | Renderer (`timeline.insert`'s `mode`) | — | WP-2A.13 |
| `editing.nudge` | Nudge step for comma and period (Shift: ×10), 1 to 100 frames | 1 frame | App | Renderer | — | WP-2A.13 |
| `editing.undo_steps` | Undo steps kept per person or agent, 1 to 10,000; a lower number drops the oldest steps at once | 1,000 | App | Engine (UI-only: `edit.set_history_limit`) | Read (`project.info`: `history_limit`) | WP-2A.13 |

### Timeline

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `timeline.snap` | Snapping on when a project opens | On | App | Renderer | — | WP-2A.13 |
| `timeline.snap_targets` | Snap to playhead, clip edges, markers (with the sequence's in and out), beats (markers of kind `beat`) | All on | App | Renderer (shared snapping) | — | WP-2A.13 |
| `timeline.snap_distance` | Snap distance, 2 to 32 px | 8 px | App | Renderer | — | WP-2A.13 |
| `timeline.follow` | Follow the playhead while playing: page, smooth, off | Page | App | Renderer | — | WP-2A.13 |
| `timeline.magnetic_main` | Main track is magnetic in new sequences. Stored, not applied yet: creating a sequence takes no such value ([ADR-0037](../adr/0037-magnetic-main-track-in-new-sequences.md), Proposed) | On | App, copied to the sequence | Renderer (sequence creation), once ADR-0037 is decided | Read and change the sequence (`track.set_magnetic`) | WP-2A.13 (the row), ADR-0037 (applying it) |
| `timeline.thumbnails` | Video clip thumbnails: filmstrip, first frame, off | Filmstrip | App | Renderer | — | WP-2A.13 |
| `timeline.waveforms` | Audio waveforms on clips | On | App | Renderer | — | WP-2A.13 |
| `timeline.track_height` | Height of new tracks: small, medium, large (tracks made by dropping or moving a clip, by Enter in the bin and by the track header's Add track; not yet the context menu's Add track above and below; medium is the engine's default height) | Medium | App | Renderer (`track.set_height`, in the same undo step) | Read and change the track (`track.set_height`) | WP-2A.13 |
| `timeline.wheel` | Mouse wheel scrolls (Ctrl/Cmd+wheel zooms) or zooms | Scrolls | App | Renderer | — | WP-2A.13 |
| `timeline.time_display` | Show time as timecode, frames or seconds (the ruler, the time under the preview and the Inspector's Start and Duration) | Timecode | App | Renderer | — | WP-2A.13 |
| `timeline.drop_frame` | Drop-frame timecode at 29.97 and 59.94 fps | On | App | Renderer | — | WP-2A.13 |

### Playback

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `playback.quality_playing` | Preview quality while playing: auto, full, half, quarter | Auto | App | Engine (quality ladder) | — | WP-2H.10 |
| `playback.quality_paused` | Preview quality when paused: full or auto | Full | App | Engine | — | WP-2H.10 |
| `playback.proxies` | Use proxies in the preview when they are ready | On | App | Engine (`view.set_preview_quality{use_proxies}`, sent after each handshake and on change) | Read (`playback.state.use_proxies`) | The key and the transport bar's toggle: WP-2F.02; this row: WP-2H.10 |
| `playback.scrub_audio` | Play audio while scrubbing | On | App | Engine | — | WP-2H.10 |
| `playback.loop` | Loop playback | Off | App | Engine | — | WP-2H.10 |
| `playback.return_to_start` | When playback stops, go back to where it started | Off | App | Renderer | — | WP-2H.10 |

### Audio

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `audio.output_device` | Output device | System default | Machine | Engine (UI-only) | Read | WP-2E.08 |
| `audio.buffer` | Output buffer size | Auto (256 to 1024 samples) | Machine | Engine (UI-only) | — | WP-2E.08 |
| `audio.input_device` | Input device for voiceover | System default | Machine | Engine (UI-only) | Read | WP-2E.08 (device list: WP-2E.04) |
| `audio.latency` | Recording latency compensation | As the device reports | Machine | Engine | — | WP-2E.08 |
| `audio.count_in` | Count-in before recording | 3 s | App | Renderer | — | WP-2E.08 |
| `audio.monitor` | Hear the microphone while recording | Off | App | Engine | — | WP-2E.08 |
| `audio.loudness_target` | Loudness target for Normalize | −14 LUFS | App | Renderer (passes it to the command) | — | WP-2E.08 (uses WP-2E.03) |
| `audio.mute_in_background` | Mute playback when Cutweave is in the background | Off | App | Renderer | — | WP-2E.08 |

### Media and import

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `media.hover_scrub` | Show a video's frames while the pointer moves across its card in the bin | On | App | Renderer | — | WP-1M.14 |
| `history.order` | History panel order: oldest first or newest first | Oldest first | App | Renderer (History panel) | — | #171 |
| `media.proxy_policy` | Proxies for new projects: off, auto (H.264 and HEVC video larger than 1920 pixels, faster than 60 fps, or 10-bit HEVC), all video | Auto | App, copied to the project | Renderer (passes it to `project.new` as `proxy_policy`) | Read and change the project's value (`media.set_proxy_policy`) | WP-2F.03b ([ADR-0027](../adr/0027-proxy-policy-in-the-project.md)) |
| `media.proxy_size` | Proxy size: 540p, 720p, 960p (the longest side of a proxy picture in pixels; a new size applies to proxies made from then on) | 960p | Machine | Engine (UI-only, `media.set_proxy_config`) | Read (`media.cache_status`) | WP-2F.03b |
| `media.keep_structure` | Keep folder structure when importing folders | On | App | Renderer (passes it to `media.import`) | — | WP-2F.09 |
| `media.copy_on_import` | Copy imported files into the project folder | Off | App | Renderer (passes it to `media.import` and `media.import_sequence` as `copy_to_project`; the project must be saved first) | — | WP-2F.04c |
| `media.image_sequence_fps` | Frame rate for image sequences: the sequence's frame rate, or 23.976, 24, 25, 29.97, 30, 50, 59.94 or 60 fps | The sequence's frame rate | App | Renderer (passes it to `media.import_sequence` as `fps`; the engine's own default is the frame rate of the sequence the editor shows, 30 without one) | — | WP-2F.05c |
| `media.transcribe_on_import` | Transcribe speech in new media | Off | App | Renderer | — | WP-3D.06 |
| `media.analyze_on_import` | Detect scenes and beats in new media | Off | App | Renderer | — | WP-5G.11 |

### Export

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `export.folder` | Save exports in: the project folder, the last folder used, or a chosen folder | Project folder | App | Renderer (export dialog) | — | WP-1M.13 |
| `export.folder_path` | The chosen folder (used when `export.folder` is "chosen"; the project folder applies until one is picked) | None | App | Renderer (export dialog) | — | WP-1M.13 |
| `export.last_folder` | The folder last picked in the Export dialog (a different folder there updates this one only; "Forget" clears it) | None | App | Renderer (export dialog) | — | WP-1M.13 |
| `export.quality` | Default quality: low, medium, high (the quality the general "H.264 MP4 1080p" preset starts at in the dialog; other presets start at their own) | Medium | App | Renderer | — | WP-1M.13 |
| `export.hw` | Use hardware encoders: auto or off (a preset made for quality, such as Archive, starts with them off) | Auto | App | Renderer (`export.start{hw}`) | — | WP-1M.13 |
| `export.preset` | Default preset: the last one used, or a preset of `export.list_presets` | Last used | App | Renderer (export dialog) | — | WP-2G.04 |
| `export.last_preset` | The preset of the last export started from the dialog (what "Last used" means) | None | App | Renderer (export dialog) | — | WP-2G.04 |
| `export.filename` | File name pattern: `{project}`, `{sequence}`, `{date}` (YYYY-MM-DD) and `{preset}` are filled in; the extension follows the preset | `{project}` | App | Renderer (export dialog) | — | WP-2G.04 |

### Saving and backups

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `saving.autosave` | Save automatically | On | App | Engine (UI-only) | Read | WP-1M.10 |
| `saving.interval` | While editing, save at least every 30 s, 1, 2 or 5 min | 30 s | App | Engine (UI-only) | Read | WP-1M.10 |
| `saving.backups` | Backups to keep: 5, 10, 20 | 10 | App | Engine (UI-only) | Read | WP-1M.10 |

Backups always go to the project's `.cutweave/backups/` (arch §17); that is not a setting.

### Keyboard shortcuts

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| — | Every command with a key, searchable; Add shortcut records a key, and a conflict asks before reassigning | The preset's keys | — | — | — | WP-1M.13, WP-2H.05 |
| `shortcuts.preset` | Preset: Cutweave (NLE-style) or CapCut | Cutweave | App | Renderer (keymap) | — | WP-2H.05 |
| `shortcuts.bindings` | Your bindings: the keys that differ from the preset, by command (`[]` removes a command's keys; the first key is the one menus show, so choosing the Redo key is moving it first). Reset all shortcuts, and Export and Import shortcuts as a file, are on the same page. Only the changes are stored, so a preset fixed in an update still reaches you | None | App | Renderer | — | WP-2H.05 |

Keys that cannot be bound: F10 and Alt+F, E, V, H (the menu bar), Shift+F10 and the Menu key (context menus), Alt+F4, Ctrl/Cmd+C, X, V, A (text fields), and Esc, Enter and Tab (dialogs). Ctrl+K opens the command palette and can be moved to another key. The macOS native menu keeps its own accelerators for the File and Edit commands.

### Notifications

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `notify.agent_connected` | Show a notice when an agent connects or disconnects | On | App | Renderer | — | WP-1J.05 |
| `notify.export_done` | Notify when an export finishes while Cutweave is in the background | On | App | Main (OS notification) | — | WP-2G.04 |
| `notify.sound` | Play a sound when an export finishes | Off | App | Renderer | — | WP-2G.04 |
| `notify.ai_done` | Notify when an AI generation finishes while Cutweave is in the background | On | App | Main | — | WP-5F.04 |

### Agents and MCP

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `agents.allowed_folders` | Folders agents can use, besides the open project's (added only with the OS folder picker in main, stored resolved; at most 64) | None | Machine (never exported or imported; the editor cannot write it) | Main (`--allow-root` at every engine start and restart; `project.set_allowed_roots`, UI-only, on change) | Read (`project.info.allowed_roots`) | WP-1J.05 |
| `agents.client_profile` | Tool set in copied client setups: minimal, standard, full (`cutweave mcp --profile`) | Standard | App | Main (builds the setups) | — | WP-1J.05 |
| — | Whether agents can connect; Connect a client (copy the Claude Code, Claude Desktop, Cursor, Codex or stdio setup: the bundled CLI's absolute path with `mcp --attach`, never a pid, port or token; main writes the clipboard); Connected agents (`project.connections`, read-only) | Information and actions | — | — | `project.connections` (read) | WP-1J.05 |
| `agents.budget_action` | Agent spending per action without asking | $0 | App | Engine (UI-only) | Read | WP-5F.07 |
| `agents.budget_session` | Agent spending per session | $0 | App | Engine (UI-only) | Read | WP-5F.07 |

### AI providers

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| — | Provider keys (write-only; fingerprint shown) | None | Secret (keychain) | Engine (`ai.set_key`, UI-only) | Never | WP-3D.06, WP-5F.07 |
| `ai.routing` | Where each kind of AI task runs: cloud first, local first, local only, off | Cloud first | App | Engine (UI-only) | Read | WP-5F.07 |
| `ai.local_only` | Local only: nothing leaves this computer | Off | App | Engine (UI-only) | Read | WP-5F.07 |
| `ai.catalog` | Model catalog updates: automatic or manual | Automatic | App | Engine | Read | WP-5F.07 |
| `ai.custom_models` | Custom model slugs | None | App | Engine | Read | WP-5F.07 |
| `ai.monthly_cap` | Monthly spending cap | None | App | Engine (UI-only) | Read | WP-5F.07 |
| — | Local models: download, licence, remove | None installed | Machine | Engine (`model.*`) | Read | WP-5F.07 |
| `ai.models_folder` | Folder for downloaded local models | The app's data folder | Machine | Engine (UI-only) | Read | WP-5I.01 |

### Copilot

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `copilot.models` | Models for the copilot: a smart one and a fast one | None until a provider is set | App | Engine | — | WP-6I.08 |
| `copilot.autonomy` | The copilot proposes changes or applies them directly | Proposes | App | Engine (UI-only) | — | WP-6I.08 |
| `copilot.budget_task` | Spending per copilot task | $0 | App | Engine (UI-only) | — | WP-6I.08 |
| `copilot.memory` | Remember preferences between sessions (view and delete in the Memory panel) | On | App | Engine | — | WP-6I.08 (memory: WP-6I.05) |

### Plugins

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| — | Installed plugins: on or off, update, roll back, remove, permissions | — | App | Engine (`plugin.*`; permission grants UI-only) | Read | WP-6C.05, WP-6B.04 |
| `plugins.auto_update` | Update plugins automatically | Off | App | Engine | — | WP-6C.05 |
| `plugins.allow_files` | Allow installing plugins from files (unverified) | Off | App | Engine (UI-only) | — | WP-6C.05 |
| `plugins.dev_mode` | Developer mode: load unpacked plugins with hot reload | Off | App | Engine (UI-only) | — | WP-6C.05 |
| `plugins.<id>.*` | Pages and rows a plugin declares | The plugin's defaults | App | The plugin | The plugin | WP-6B.03 |

### Privacy

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `privacy.crash_reports` | Send crash reports: off, ask, always | Off | App | Main and engine | — | WP-3I.05 |
| — | What leaves this computer (statement; links to AI providers and Updates) | — | — | — | — | WP-3I.05 |
| `privacy.update_checks` | Check for updates | On | App | Main | — | WP-7A.06 |

Cutweave collects no usage analytics, so there is no setting for it (product brief, principle 8).

### Updates

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `updates.channel` | Update channel: stable, beta, nightly | Stable | App | Main | — | WP-7A.06 |
| `updates.auto_download` | Download updates automatically (installed when idle, never during an export) | On | App | Main | — | WP-7A.06 |
| — | Check now, skipped versions | — | — | — | — | WP-7A.06 |

### Cache and storage (this computer)

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `cache.folder` | Cache folder, chosen with the OS folder picker (applies at the next start; the old folder is left for the person to delete) | The OS cache folder | Machine | Main (`--cache-dir` at engine start) | Read (`media.cache_status`) | WP-1D.08 |
| `cache.thumbnails_limit` | Thumbnails and filmstrips: 512 MiB, 1, 2, 5, 10 or 25 GiB (lowering it deletes the oldest at once) | 2 GiB | Machine | Engine (UI-only, `media.set_cache_config`) | Read | WP-1D.08 |
| `cache.waveforms_limit` | Waveforms: 128, 256, 512 MiB, 1 or 2 GiB | 512 MiB | Machine | Engine (UI-only, `media.set_cache_config`) | Read | WP-1D.08 |
| `cache.proxies_limit` | Proxies: 10, 25, 50, 100, 250 or 500 GiB (lowering it deletes the proxies used longest ago at once), with its usage and "Clear proxies" | 50 GiB | Machine | Engine (UI-only, `media.set_cache_config`) | Read | WP-2F.03b |
| `cache.render_limit` | Background renders | 20 GiB | Machine | Engine (UI-only) | Read | WP-2J.09 |
| — | Usage per kind, Clear one kind, Clear all (asks first) | — | — | Engine (UI-only, `media.clear_cache`) | Read (usage) | WP-1D.08 |

### Performance and hardware (this computer)

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| `performance.hw_decode` | Hardware decoding: auto or off (applies to decoders opened from then on; open pools reopen on their next seek) | Auto | Machine | Engine (UI-only, `media.set_decode_policy`) | Read | WP-1D.08 |
| `performance.frame_memory` | Memory for decoded frames: Auto, 512 MiB, 1, 2 or 4 GiB | Auto (the smaller of 1 GiB and a quarter of RAM) | Machine | Engine (UI-only, `media.set_cache_config`) | Read | WP-1D.08 |
| — | GPU in use, hardware encoders and hardware decoders found (information) | — | — | — | Read (`media.capabilities`) | WP-1D.08 |
| `performance.proxy_jobs` | Proxy jobs at once: 1 to 4 (one while an export runs) | 2 | Machine | Engine (UI-only, `media.set_proxy_config`) | Read (`media.cache_status`) | WP-2F.03b |
| `performance.background_delay` | Start background rendering after this much idle time | 1.5 s | Machine | Engine (UI-only) | Read | WP-2J.09 |
| `performance.pause_on_battery` | Pause background work on battery | On | Machine | Engine (UI-only) | Read | WP-2J.09 |

### Advanced

| Key | Setting | Default | Scope | Applied by | Agents | Ships in |
|---|---|---|---|---|---|---|
| — | Export settings…, Import settings… (App rows only) | — | — | Main | — | WP-1M.11 |
| — | Reset all settings… | — | — | Main | — | WP-1M.11 |
| — | Open the settings folder, open the logs folder | — | — | Main | — | WP-1M.11 |

## Project settings (File > Project settings…, WP-2H.08)

Stored in the document, changed with undoable commands (`project.set_settings`, or the feature's own command), visible to agents through `project.info` (`settings`).

`project.set_settings{patch}` takes any of the keys below as a merge patch: a key left out keeps its value, `null` puts an optional key back to its default, an object (`default_format`) merges key by key, and a key the settings do not have fails with `invalid` at its path. The feature commands that set one key (`media.set_proxy_policy`) send the same patch through the same checks. `project.new` takes the same object as `settings` (next to `format`, `still_duration` and `proxy_policy`, which name one key each; a key given both ways is refused). The dialog is drawn from the generated schema of the patch, so a key a later work package adds to `ProjectSettings` gets a row there without UI work. "Use as defaults for new projects" copies the values that have an app preference (`editing.still_duration` in whole seconds, `media.proxy_policy`, and `default_format` as `editing.sequence_format`). The format copied is the project's format of new sequences, the one this dialog shows, not the format of any one sequence; File > New project then passes it to `project.new` as `settings.default_format`, so the new project's first sequence and later ones start in it.

| Key | Setting | Default | Agents | Ships in |
|---|---|---|---|---|
| `meta.name` | Project name | Chosen at creation | Read and change (`project.rename`) | Exists (P1) |
| `default_format` | Format of new sequences | 1920 × 1080, 30 fps, 48 kHz | Read and change (`project.set_settings`) | WP-2H.08 (field exists since WP-1A.02) |
| `still_duration` | Still image duration | The app default (5 s) | Read and change (`project.set_settings`) | The field and the insert default: WP-2F.05b ([ADR-0035](../adr/0035-still-duration-and-image-sequences-in-the-project.md)); the row in File > Project settings…: WP-2H.08 |
| `proxy_policy` | Proxies: off, auto, all (each asset can also be forced or turned off: `media.set_proxy_mode`) | The app default (auto) | Read and change (`media.set_proxy_policy`, `project.set_settings`) | The field and the commands: WP-2F.03b ([ADR-0027](../adr/0027-proxy-policy-in-the-project.md)); the row in File > Project settings…: WP-2H.08 |
| `render_cache.enabled` | Render heavy parts in the background | On | Read and change | WP-2J.09 |
| `autosave` | Autosave for engines without the app (headless, agents) | On, every 30 s | Read and change | WP-1C.03 (dialog row: WP-2H.08) |
| `ai_budget` | AI spending cap for this project | None | Read only (changing budgets is UI-only, ADR-0016) | WP-5A.07 (dialog row: WP-2H.08) |
