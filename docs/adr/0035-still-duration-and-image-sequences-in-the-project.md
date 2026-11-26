# ADR-0035: Store the still duration in the project and add image sequences as a media kind

- **Status:** Proposed
- **Date:** 2026-11-26
- **Deciders:** Cutweave maintainers, on the review of WP-2F.05a (before the follow-ups that add the fields and commands)
- **Supersedes:** none
- **Architecture refs:** `docs/01-architecture.md` §4.2, §4.3, §5.1, §7, §17

## Context

WP-2F.05 ([P2 WBS](../wbs/P2-core-editor.md)) covers still images, image sequences and the length a still gets on the timeline. It was split: WP-2F.05a ships what needs no contract change (PNG, JPEG, WebP, TIFF and BMP stills import through `media.import`, the EXIF orientation is honoured, 16-bit and transparent pictures composite correctly). The rest changes the project format and the command surface, which ADR-0006 and AGENTS.md say need an ADR:

- `project.settings.still_duration`: today a still inserted without a duration gets a fixed 5 s (`cutweave_ops::timeline::DEFAULT_STILL_SECS`). The plan makes it a project setting, with the app preference `editing.still_duration` copied into new projects ([settings.md](../reference/settings.md)).
- **Image sequences:** a numbered run of pictures (`shot_0001.exr` … `shot_0300.exr`, a render, a time-lapse) imported as one asset that plays like video. `MediaKind` has `video`, `audio`, `image` and `sequence_ref`; it is a closed enum, so a build that does not know a new kind cannot open a document that uses it.
- Three commands: `media.import_sequence`, `media.import_images_as_slideshow` and `media.import_frame`.
- Two settings rows: `editing.still_duration` and `media.image_sequence_fps`.

## Decision drivers

- Changing a default must never alter clips that already exist: a clip's length is its own data.
- Older builds must not misread a newer document: a sequence shown as its first picture only, saved again and silently broken, is worse than a clear "this project needs a newer Cutweave".
- Files stay referenced, never copied or rewritten (arch §17); a sequence is hundreds of files that can go missing one by one.
- Every import path is a command an agent can call too (ADR-0002, ADR-0005), and import stays fast: probing 300 pictures must not decode 300 pictures.
- Time is flicks (ADR-0004): a sequence's frame rate is a rational `FrameRate`, never a float.

## Considered options

### The still duration

1. **An optional project field, no schema bump (leading):** `ProjectSettings.still_duration: Duration`, default 5 s, left out of the file when it is the default and read as 5 s when absent (the pattern of ADR-0027). `timeline.insert`, `timeline.overwrite` and `sequence.create_from_media` use it when no `duration` is given. `project.set_settings` (WP-2H.08) patches it; `project.new` copies the app preference `editing.still_duration` into it.
2. **An app preference only:** no format change, but the same project would cut differently on two machines, and an agent could not read the value it is about to rely on from the document.
3. **A required `duration` on every insert of a still:** explicit, but every caller (the UI drag, agents, the CLI) would have to invent the number.

### Image sequences

1. **A new media kind and a description of the run, with a schema version bump (leading):** `MediaKind::ImageSequence` (`"image_sequence"`) and `MediaAsset.image_sequence: { pattern, first, count, fps, missing[] }`. `path_abs` / `path_rel` name the first file; `pattern` is the file name with the counter as `%0Nd` (`shot_%04d.exr`); `first` is the first number; `count` the numbers covered; `fps` the frame rate chosen at import; `missing` the numbers with no file at import time. The probe has a `duration` of `count / fps` and the picture size of the first file. The schema version goes up by one with a migration that changes nothing (older documents have no sequences), so an older build refuses the document instead of failing on an unknown enum value or dropping the run.
2. **`kind = video` plus the optional `image_sequence` field, no schema bump:** older builds open the document, but they keep the field in `extra` and decode the asset as its first file: a one-frame video. Clips cut from the sequence then show one picture, with no error. Rejected as the leading option for that silent misread.
3. **A sequence of this project made of still clips (no new kind):** works with today's model, and it is exactly what the slideshow command builds, but 300 one-frame clips per sequence are slow to edit and carry no shared identity (relink, proxies and the bin would see 300 assets).
4. **Transcode the run to a video file at import:** no format change, but it copies media, loses precision (EXR, 16-bit TIFF) and breaks the "files are referenced" rule.

## Decision

**Decided by:** the maintainers' review of WP-2F.05a. Nothing below is built until this ADR is accepted. Pass criteria for the follow-up work packages:

- an older document opens with a 5 s still duration; a document with the field opens in an older build and keeps it in `extra`; changing the setting changes the next inserted still and no existing clip (a property test over insert, change, insert);
- a document with an image sequence is refused by a build with the older schema version with the usual "newer version" message, and the migration golden passes;
- a 300-frame PNG sequence imports in under 3 s (probe only), plays at 30 fps and every frame is exact (frame-number burn-in);
- the three commands are undoable where they change the document, have parity tests (UI, MCP, CLI), and `media.import_frame`'s picture equals `render.frame` at that time (golden).

If accepted, we will:

1. **Still duration.** Add `ProjectSettings.still_duration` (option 1). A still inserted without a `duration` gets it, rounded to whole frames of the sequence, at least one frame. Existing clips never change. Settings rows: `editing.still_duration` (Settings > Editing, the default for new projects) and the project's own row in File > Project settings… (WP-2H.08). The bin's "Duration…" item for image cards (context menu, ADR-0026) sets the length the *next* insert of that card uses in the session; it is UI state, not a model field.
2. **Image sequences.** Add the kind and the field of option 1 and bump the schema version.
   - **Detection.** `media.import` never guesses: one picture stays one still. `media.import_sequence{ path | folder, fps?, folder? }` takes one file of a run (the run is every file of the same directory that differs only in the trailing number, with the same extension) or a directory (its pictures in numeric order, one extension, one run). Fewer than two files is `Invalid`. The UI offers it when a dropped or picked selection is a run of two or more ("Import as image sequence").
   - **Gaps.** Missing numbers are allowed and reported: the command's `warnings` name the ranges, the asset keeps them in `missing`, and a missing frame shows the last picture before it (flagged like a damaged frame, ADR-0003). A file that disappears later is the ordinary missing-media case (WP-2F.03), per frame.
   - **Frame rate.** `fps` if given, else the app setting `media.image_sequence_fps` (Settings > Media and import), whose default is "the active sequence's frame rate" (30 fps without one). It is stored on the asset; changing the setting never changes an imported asset.
   - **Decoding.** The media backend gets the run as a pattern and a start number (FFmpeg's `image2` demuxer); the frame index is computed, not scanned (frame `n` is file `first + n`), so import probes one file. Sequences are video for tracks, proxies and export; they have no audio.
   - MCP: `media_import_sequence`, catalog.
3. **`media.import_images_as_slideshow{ files[], per_image?, order?, sequence?, track?, at? }`.** Imports the pictures as stills (existing assets are reused by content hash) and places them one after the other, each `per_image` long (default: the project's `still_duration`), in `order`: `given` (default), `name` or `name_numeric`. One undo step. It builds on today's model only and needs no format change; it is listed here because it is a new command. MCP: catalog.
4. **`media.import_frame{ source: "sequence" | { media }, at, name?, folder? }`.** Renders the frame the Preview (`sequence`) or the Source viewer (`media`) shows at `at`, at full sequence or source size, writes it as a PNG into the project's `generated/frames/` folder and registers it as an image asset in the bin. Its pixels equal `render.frame` at that time. One undo step removes the asset; the file stays until the project's generated files are cleaned. It is the Preview's "Add frame to media bin" (ADR-0026). MCP: catalog.

**Fallback** (rejected or deferred): stills keep the fixed 5 s (callers pass `duration` to choose another), image sequences are not importable as one asset (a run can be turned into a video outside Cutweave), and the two convenience commands wait.

## Consequences

**Positive**

- A project carries its editor's still length; agents can read and change it like any other project setting.
- Renders and time-lapses come in as one asset with exact frames, relink and proxies, without copying media.
- "Add frame to media bin" and slideshows become one command each, the same for people and agents.

**Negative / costs**

- A schema version bump: a permanent no-op migration and its golden fixture, chained after the versions already on `main`; older builds cannot open projects saved after it (they say so).
- One more media kind for every `match` on `MediaKind` (render evaluation, track rules, the bin's filters and icons, proxies, relink, collect files).
- Per-frame missing-file handling in relink (WP-2F.03) and collect (WP-2F.04) for sequences.
- `media.import_frame` writes a file as part of a command: the command must stay deterministic for a replay (the file name derives from the new media id).

**Follow-ups**

- WP-2F.05b: `still_duration` (field, insert default, settings rows, "Duration…" for image cards).
- WP-2F.05c: image sequences (kind, field, schema bump, `media.import_sequence`, backend pattern input, gap warnings, the import prompt).
- WP-2F.05d: `media.import_images_as_slideshow` and `media.import_frame`.
- Not decided here: GIF and animated WebP/APNG as video, SVG, HEIC, RAW stills, and the mirrored EXIF orientations (2, 4, 5, 7), which need a flip the probe cannot express today.

## Links

- Architecture: [§4.2, §4.3, §5.1, §7, §17](../01-architecture.md)
- Work breakdown: [WP-2F.05, WP-2F.03, WP-2F.04, WP-2H.08](../wbs/P2-core-editor.md); settings: [settings.md](../reference/settings.md); context menus: [09-context-menus.md](../research/09-context-menus.md)
- Related: [ADR-0006](0006-project-format.md) (format changes need an ADR), [ADR-0020](0020-media-folders.md) (schema v2), [ADR-0026](0026-context-menu-surfaces-and-contributions.md), [ADR-0027](0027-proxy-policy-in-the-project.md) (optional settings fields without a schema bump)
