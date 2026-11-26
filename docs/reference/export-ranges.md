# Export ranges, file names and the disk check

What `export.start` and `export.enqueue` accept beyond a path and a preset (WP-2G.03b). Both take
the same params. The queue itself is described in [ADR-0030](../adr/0030-export-queue-in-the-project.md);
the architecture is in [01-architecture.md](../01-architecture.md) (§7, §8).

## `range`

`{kind, in, out, markers, pair_mode}`; every field is optional.

| `kind` | Needs | Exports |
|---|---|---|
| `sequence` (default) | nothing | the whole sequence |
| `in_out` | the sequence's in and out points (`timeline.set_in_out`) | `[in, out)`; an out point past the end stops at the end |
| `custom` | `in` and `out` | `[in, out)` (`start` / `end` are accepted for `in` / `out`) |
| `markers` | optionally `markers` (marker ids; default all of the sequence's markers) | one file per marker |

Without `kind`: `markers` means `markers`, `in` and `out` mean `custom`, nothing means `sequence`.
A range of just `{in, out}` therefore means what it did before. Times are frame-aligned (the
nearest frame) and `out` is exclusive.

**Marker sets** queue one export job per region, in time order. `pair_mode: "duration"` (default)
exports each marker's own `duration`; markers without one are skipped when you ask for all markers
and are an error when you name them. `pair_mode: "pairs"` exports each marker up to the next one,
so `n` markers give `n - 1` files. A region shorter than a frame is an error. The result lists
every file in `jobs` (`job_id`, `path`, `label`, `in`, `out`, `frames`, `duration`); the top-level
fields describe the first. If one of the jobs cannot be queued none of them is.

## File names for a marker set

`path` is an existing folder (files are named `{seq}_{marker_label}_{n}.mp4`) or an `.mp4` path
whose file name is the template. Tokens: `{seq}` (the sequence's name), `{marker_label}` (the
marker's label; `marker` when empty) and `{n}` (1, 2, … in time order). Characters a file name
cannot hold become `_`. With more than one file the template needs `{n}` or `{marker_label}`.

## `collision`

What happens when the output file already exists, or another queued or running export writes it.

| Value | Existing file | File of another unfinished export |
|---|---|---|
| `fail` (default) | `Invalid` at `/path` (at `/overwrite` when you passed neither `collision` nor `overwrite`) | `Invalid` at `/path` |
| `rename` | a free name is used: `name (1).mp4`, `name (2).mp4`, … | the same |
| `overwrite` | replaced, but only when the export succeeds; a failed or cancelled export leaves it | `Invalid` at `/path` |

`overwrite: true` is the same as `collision: "overwrite"`; giving it with another `collision` is
`Invalid` at `/overwrite`. The name is chosen when the export is queued, and the output's `path`
says which. All the files of one marker set are checked together, so two regions never get the
same name.

## Disk check

Before queueing, each output's size is estimated (the preset's bitrate, or a typical bitrate for
its quality, plus 192 kbit/s audio, plus 5 % for the container) and compared with the free space
of its drive; the files of one call on the same drive count together. Too little room is `Invalid`
at `/path`, saying what is needed and what is free. The estimate is an upper-end guess for
typical footage. When the system cannot report the free space the check is skipped.
