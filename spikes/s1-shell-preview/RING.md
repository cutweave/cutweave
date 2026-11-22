# S1 preview frame ring: layout v1 (`CWS1`)

Interface document for WP-0C.01, feeding WP-1H.02 (`cutweave-bridge` / `cutweave-playback`).
Arch §10 step 1. Source of truth for the spike: `ring/src/layout.rs` and `ring/src/ring.rs`.

One **named shared-memory object** per engine launch, one writer (the engine), any number
of readers (the napi addon, in the main editor window's preload). Latest-wins: the writer
never waits for readers. All integers are little-endian. Every field that both sides touch is
accessed atomically (8-byte fields are 8-byte aligned).

## Naming and lifetime

| | Windows (implemented) | Linux / macOS (stubbed in the spike) |
|---|---|---|
| Name | `Local\cw-s1-<12 hex>` (per-session namespace) | `/cw-s1-<12 hex>` (≤ 30 bytes, macOS limit 31) |
| Create | `CreateFileMappingW` (pagefile-backed); **fails if the name already exists** (anti-squatting) | `shm_open(O_CREAT\|O_EXCL, 0600)` + `ftruncate` + `mmap` |
| Open | `OpenFileMappingW(FILE_MAP_READ)` + `MapViewOfFile` (read-only view) | `shm_open(O_RDONLY)` + `fstat` + `mmap(PROT_READ)` |
| Cleanup | Kernel object dies with its last handle. After an engine crash the preload closes its view on `engine-down`. | Creator `shm_unlink`s on clean exit; Electron main (which chose the name) `shm_unlink`s after observing a crash. |

Electron main chooses the name (`--ring`), so it can clean up even after `kill -9`.

## Mapping layout

```text
offset 0      RingHeader          256 B
offset 256    SlotDesc[N]         128 B each, N = slot_count (4..=6)
offset 4096   payload[0]          slot_stride bytes (slot_capacity rounded up to 4 KiB)
              payload[1] ... payload[N-1]
total = 4096 + N * slot_stride
```

### RingHeader (offset 0, 256 B)

| Off | Type | Field | Notes |
|---:|---|---|---|
| 0 | u32 | `magic` | `"CWS1"` = `0x31535743`; stored **last** at init (Release) |
| 4 | u32 | `version` | 1 |
| 8 | u32 | `header_size` | 4096 |
| 12 | u32 | `slot_count` | 4..=6 |
| 16 | u64 | `slot_capacity` | max payload bytes per slot (spike: W·H·4 of the largest frame, `--max-width` x `--max-height`, default 2560x1440 = 14.7 MB) |
| 24 | u64 | `payload_offset` | 4096 |
| 32 | u64 | `slot_stride` | `align_up(slot_capacity, 4096)` |
| 40 | u64 | `write_index` | frames published so far; newest frame is in slot `(write_index - 1) % N`; 0 = none. Stored with Release **after** the slot is complete |
| 48 | i64 | `audio_clock_flicks` | engine's (fake) audio clock, updated continuously (~1 kHz) |
| 56 | u64 | `audio_clock_host_us` | host time when `audio_clock_flicks` was sampled; readers extrapolate `clock(now) = flicks + (now_us - host_us) * 705.6` while playing |
| 64 | u64 | `heartbeat_us` | host time of the engine's last loop iteration (hang detection) |
| 72 | i64 | `frame_duration_flicks` | 11 760 000 at 60 fps |
| 80 | u32 | `engine_pid` | |
| 84 | u32 | `engine_state` | 0 starting, 1 playing, 2 paused, 3 stopped |
| 88 | u64 | `engine_dropped` | frames the producer skipped (late or no free readback buffer) |
| 96..256 | | reserved | zero |

"Host time" everywhere is **µs since the Unix epoch** (`GetSystemTimePreciseAsFileTime` on
Windows) so engine, addon and renderer timestamps are directly comparable. Time values in
the media domain are flicks (705 600 000 per second, arch §4.1).

### SlotDesc (offset 256 + 128·i)

| Off | Type | Field | Notes |
|---:|---|---|---|
| 0 | u64 | `lock` | **seqlock**: odd while the writer fills the slot, even when stable |
| 8 | u64 | `seq` | frame sequence number = `write_index` right after publish (1-based, strictly increasing across slots) |
| 16 | i64 | `pts` | presentation timestamp (flicks) |
| 24 | u32 | `width` | pixels |
| 28 | u32 | `height` | pixels |
| 32 | u32 | `format` | 1 = RGBA8, 2 = NV12 (BT.709 limited range) |
| 36 | u32 | `stride` | bytes per row of plane 0 (tightly packed in the spike) |
| 40 | u64 | `rev` | render revision (project revision the frame was rendered from) |
| 48 | u64 | `payload_len` | bytes used (RGBA8: `stride·h`; NV12: `stride·h + stride·h/2`, UV plane follows Y) |
| 56 | u32 | `checksum` | see Integrity |
| 60 | u32 | `flags` | bit 0 `LOWRES`: reduced-size scrub preview; a full-resolution frame of the same `pts` follows unless a newer seek supersedes it (WP-0C.02). Other bits 0 |
| 64 | u64 | `t_render_us` | host time the frame's GPU work was submitted |
| 72 | u64 | `t_publish_us` | host time the slot was published |
| 80..128 | | reserved | zero |

## Protocol

Writer, per frame (`s = write_index % N`):

1. `lock = lock | 1` (Relaxed), Release fence.
2. Copy pixels into `payload[s]`, stamp rows, compute checksum, store descriptor fields (Relaxed).
3. `lock = (lock | 1) + 1` (Release).
4. `write_index += 1` (Release).

Reader (latest-wins): `wi = write_index` (Acquire); slot `(wi - 1) % N`; `l1 = lock` (Acquire,
must be even); copy descriptor + payload; Acquire fence; `l2 = lock`; accept iff `l1 == l2`,
else retry (3 tries), then report "torn". A reader that wants a specific pts (the 0C.02
scheduler) reads the whole slot table and picks by `pts`/`seq` with the same per-slot check.
With N slots a reader has about (N − 1) frame periods to finish a copy before it can be torn.

## Integrity (spike instrumentation, not part of the product contract)

- **Row stamps:** the first 8 bytes of every plane-0 row hold `seq` (u64 LE), written by the
  writer after the pixels. A torn copy shows rows with a different `seq`.
- **Checksum:** word-wise FNV-1a (offset `0x811c9dc5`, prime `0x01000193`) over every 61st
  little-endian u32 of the payload (`i = 0, 61, 122, ...`), then `h = (h ^ payload_len) * prime`.
  Known vector: 4096 bytes where word `i` = `7·i` → `0x1b54ff1d`. Mirrors:
  `ring/src/checksum.rs` (Rust) and `desktop/src/shared/integrity.ts` (TS).

## Notes for WP-1H.02

- Keep the layout; drop row stamps/checksum (or keep behind a debug flag).
- `slot_capacity` should be sized for the largest preview canvas you will allow; resizes
  change `width/height/stride` per slot without remapping. WP-0C.02 does exactly this: the
  renderer sends `resize {width, height}` (canvas device pixels) on the control channel, the
  engine clamps to the capacity (keeping the aspect ratio) and rounds to width % 4 == 0 and an
  even height (NV12 packing), and readers must accept any frame size per slot. Frames rendered
  before the resize stay valid; the presenter scales them. 6 slots at 2560x1440 RGBA8 is
  88 MB of pagefile-backed memory; a product ring may want fewer or smaller slots plus a
  "too large, scaled" path.
- `flags` bit 0 (`LOWRES`) marks scrub previews (arch §10 step 5). A scrub request is answered
  by the first frame with the requested `pts`, whatever its resolution.
- Consider 128-byte alignment of `write_index` / clock words onto separate cache lines if
  profiling shows false sharing (not observed in the spike).
- Zero-copy `ArrayBuffer` views over the mapping are impossible in Electron (V8 memory
  cage: `napi_no_external_buffers_allowed`); see README "Findings".
