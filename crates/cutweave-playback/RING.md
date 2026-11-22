# Preview frame ring: `RING_LAYOUT_V1`

Normative description of the shared-memory ring between the engine (writer,
`cutweave-playback`, WP-1H.02) and the Electron preload addon (reader, `cutweave-bridge`,
WP-1H.03). Architecture: [§10](../../docs/01-architecture.md), transport (a) of
[ADR-0001](../../docs/adr/0001-desktop-shell.md). It replaces the spike layout `CWS1`
([spike RING.md](../../spikes/s1-shell-preview/RING.md)) and keeps what Spike S1 proved: a
named mapping, 4–6 slots, a per-slot seqlock, latest-wins, a continuously published audio
clock and clock-scheduled frame choice.

Three artefacts carry the contract and must change together:

| Artefact | Role |
|---|---|
| this file | normative text |
| [`src/shm/layout.rs`](src/shm/layout.rs) | Rust structs and constants (source of truth for offsets) |
| [`include/cutweave_preview_ring_v1.h`](include/cutweave_preview_ring_v1.h) | C mirror; a unit test checks every `#define` against the Rust constants |

## Versioning

- `magic` is `"CWPR"` (`0x52505743` as a little-endian `u32`).
- `layout_version` is `1`. Any change an older reader would misinterpret bumps it, and v1
  readers refuse other versions.
- `required_features` is `0` in v1. A writer sets a bit only for an addition that readers must
  understand; a reader refuses a ring with any bit it does not know.
- Additions that old readers can ignore use reserved bytes, which are always zero in v1.
- Readers use the geometry fields of the header (`header_size`, `slot_desc_size`,
  `payload_offset`, `slot_stride`), not the v1 constants.

## Conventions

- Little-endian, naturally aligned; every field both sides touch is accessed atomically
  (C11 atomics, `std::atomic_ref`, Rust atomics). Payload bytes are plain memory guarded by
  the slot seqlock.
- One writer (the engine), any number of readers. Readers map the object read-only and never
  write to it. The writer never waits for a reader (latest wins).
- Times on the media timeline are flicks (`i64`, 705 600 000 per second, arch §4.1).
- `*_ns` fields are host monotonic nanoseconds of the clock named by `host_clock`:

  | `host_clock` | OS | Reader computes "now" as |
  |---:|---|---|
  | 1 | Windows | `QueryPerformanceCounter() * 1e9 / QueryPerformanceFrequency()` (128-bit, truncating) |
  | 2 | Linux | `clock_gettime(CLOCK_MONOTONIC)` |
  | 3 | macOS | `clock_gettime(CLOCK_UPTIME_RAW)` |

## Mapping

```text
0                RingHeader        1024 B (header_size)
1024             SlotDesc[N]       128 B each (slot_desc_size), N = slot_count
payload_offset   payload[0]        slot_stride bytes; v1 writers: offset 16384, stride a multiple of 16384
                 payload[1] .. payload[N-1]
mapping_size   = payload_offset + N * slot_stride
```

Payload `i` starts at `payload_offset + i * slot_stride` and holds up to `slot_capacity`
bytes. v1 writers create 4 to 6 slots (default 6) and page-align every payload (16 KiB, the
arm64 macOS page size).

### RingHeader

Five 128-byte blocks, so the frame publisher's and the clock writer's hot words never share
a cache line.

| Off | Type | Field | Meaning |
|---:|---|---|---|
| **A** | | | **Identity and geometry; immutable once `magic` is set** |
| 0 | u32 | `magic` | `CWPR_MAGIC`; stored last at initialisation (Release) |
| 4 | u32 | `layout_version` | 1 |
| 8 | u32 | `required_features` | 0 |
| 12 | u32 | `header_size` | offset of the slot table (1024) |
| 16 | u32 | `slot_desc_size` | 128 |
| 20 | u32 | `slot_count` | number of slots |
| 24 | u64 | `payload_offset` | offset of payload 0 (multiple of 4096) |
| 32 | u64 | `slot_stride` | distance between payloads |
| 40 | u64 | `slot_capacity` | largest `payload_len` (≤ `slot_stride`) |
| 48 | u64 | `mapping_size` | bytes the writer created |
| 56 | u64 | `ring_epoch` | random nonce; must equal the announced epoch |
| 64 | u32 | `writer_pid` | engine process id |
| 68 | u32 | `host_clock` | see Conventions |
| 72..128 | | reserved | 0 |
| **B** | | | **Writer progress and liveness** |
| 128 | u64 | `write_index` | frames committed; `seq` of the newest frame, in slot `(write_index - 1) % slot_count`; 0 = none (Release/Acquire) |
| 136 | u32 | `writer_state` | 0 initialising, 1 live, 2 retired, 3 closed (Release/Acquire) |
| 144 | u64 | `heartbeat_ns` | last publish or heartbeat |
| 152 | u64 | `frames_dropped` | producer-side drops (diagnostics) |
| 160..256 | | reserved | 0 |
| **C** | | | **Audio clock; seqlock `clock_lock`** |
| 256 | u64 | `clock_lock` | odd while a sample is being written |
| 264 | i64 | `clock_flicks` | audio clock position on the timeline |
| 272 | u64 | `clock_host_ns` | host time of that position |
| 280 | i32 | `clock_rate_milli` | rate in thousandths: 1000 = 1x, -2000 = 2x reverse |
| 284 | u32 | `clock_state` | 0 stopped/paused, 1 playing, 2 scrubbing |
| 288..384 | | reserved | 0 |
| **D** | | | **Negotiated preview configuration; seqlock `config_lock`** |
| 384 | u64 | `config_lock` | odd while being written |
| 392 | u32 | `config_gen` | generation, bumped by every renegotiation (starts at 1) |
| 396 | u32 | `preview_width` | canvas width in device pixels the engine renders at |
| 400 | u32 | `preview_height` | canvas height |
| 404 | u32 | `preview_format` | 1 RGBA8, 2 NV12 |
| 408..512 | | reserved | 0 |
| **E** | | | **Successor; valid once `writer_state == 2`** |
| 512 | u64 | `successor_epoch` | epoch of the ring that replaced this one |
| 520 | u8[32] | `successor_name` | its name, ASCII, NUL padded |
| 552..1024 | | reserved | 0 |

### SlotDesc (at `header_size + i * slot_desc_size`)

| Off | Type | Field | Meaning |
|---:|---|---|---|
| 0 | u64 | `lock` | seqlock: odd while the writer fills the slot |
| 8 | u64 | `seq` | frame sequence number; 0 = slot empty |
| 16 | i64 | `pts` | presentation timestamp (flicks) |
| 24 | i64 | `duration` | frame duration (flicks), 0 = unknown |
| 32 | u64 | `rev` | project revision the frame was rendered from |
| 40 | u32 | `width` | pixels, 1..=8192 |
| 44 | u32 | `height` | pixels, 1..=8192 |
| 48 | u32 | `format` | 1 RGBA8, 2 NV12 |
| 52 | u32 | `flags` | bit 0 `LOWRES`: reduced-size scrub preview, a full-size frame of the same `pts` follows unless a newer seek supersedes it (arch §10 step 5) |
| 56 | u32 | `config_gen` | configuration the frame was rendered for |
| 64 | u64 | `payload_len` | bytes in use |
| 72 | u64 | `plane0_offset` | offset of plane 0 in the payload |
| 80 | u64 | `plane1_offset` | offset of plane 1 (NV12 UV), 0 for RGBA8 |
| 88 | u32 | `plane0_stride` | bytes per row of plane 0 |
| 92 | u32 | `plane1_stride` | bytes per row of plane 1, 0 for RGBA8 |
| 96 | u64 | `render_ns` | host time the GPU work was submitted, 0 = unknown |
| 104 | u64 | `publish_ns` | host time the slot was committed |
| 112..128 | | reserved | 0 |

### Pixel formats

- **RGBA8** (1): one plane, 4 bytes per pixel in R, G, B, A order, sRGB-encoded, full range,
  opaque.
- **NV12** (2): plane 0 is Y (`height` rows of `width` bytes); plane 1 is interleaved U,V
  (`height / 2` rows of `width` bytes). BT.709, limited range. Width and height are even.
  ADR-0001 records NV12 + `writeTexture` as the Phase 1 upload path.

Rows may be padded: `stride ≥ width * bytes_per_pixel`. The engine publishes its GPU readback
buffer as is (wgpu pads rows to 256 bytes), so the copy into the ring is a single `memcpy`.
A reader validates every descriptor before using it:

- `width`, `height` in `1..=8192`, even for NV12; known `format`;
- each plane's stride ≥ its row size, and `offset + stride * rows` within `payload_len`;
- NV12 planes do not overlap; `payload_len` equals the end of the last plane and is at most
  `slot_capacity`.

Frames may have any size up to the capacity: a slot can hold a frame from an older
configuration or a low-res scrub frame. Presenters scale; `config_gen` tells whether a frame
matches the current configuration.

## Writer protocol

Initialisation: create the object (zero filled), write blocks A–E, set `writer_state = 1`,
then store `magic` with Release. A reader that sees `magic` (Acquire) sees a complete header.

Per frame, `s = write_index % slot_count`, `seq = write_index + 1`:

1. `lock = lock | 1` (Relaxed), then a Release fence.
2. Copy the payload; store the descriptor fields (Relaxed).
3. `lock = (lock | 1) + 1` (Release).
4. `write_index = seq` (Release); `heartbeat_ns = max(heartbeat_ns, now)`.

An abandoned write sets `seq = 0`, `payload_len = 0` and makes `lock` even: the slot is empty
and `write_index` is unchanged.

Clock and configuration blocks use the same seqlock: make the lock odd (compare-exchange, so
several clock-writer threads serialise), Release fence, store the fields, make it even
(Release). The engine stores clock samples at ≥ 200 Hz while playing (1 kHz by default) and
calls the heartbeat at ≥ 10 Hz while idle.

## Reader protocol

Open and validate once:

1. Open the object read-only by name (see Naming); map all of it.
2. `magic` (Acquire) must be `CWPR_MAGIC`, `layout_version` 1, no unknown `required_features`.
3. `slot_desc_size == 128`, `slot_count ≥ 1`, `header_size + slot_count * 128 ≤
   payload_offset`, `payload_offset % 4096 == 0`, `slot_capacity ≤ slot_stride`,
   `payload_offset + slot_count * slot_stride == mapping_size ≤` mapped size.
4. `ring_epoch` must equal the epoch the engine announced (rejects a stale or foreign object).
5. `host_clock` must be the clock this reader uses.

Read the newest frame (latest wins):

```text
wi = write_index (Acquire); if wi == 0: empty; if wi <= last_seq: nothing new
s  = (wi - 1) % slot_count
l1 = slot[s].lock (Acquire); if odd: retry
copy and validate the descriptor; if seq == 0: empty slot
copy payload_len bytes of payload[s]
Acquire fence; l2 = slot[s].lock (Relaxed)
accept iff l1 == l2 (else retry, a few times, then report busy)
```

A copied frame with `seq > last_seq` is valid even if it is not `wi` (the writer lapped the
reader during the copy). With N slots a reader has about N − 1 frame periods to finish a copy.

Clock-scheduled choice (arch §10 step 3; S1: 0 skipped frames at 60 fps with 20 ms latency):
read every descriptor under its lock, pick the highest `seq` with
`pts ≤ clock(now) − latency`, then copy that slot as above and check `seq` again.

Clock: read `clock_lock` (Acquire, must be even), the four fields, Acquire fence, re-read the
lock; retry until equal. Then:

```text
clock(now) = clock_flicks                                  if clock_state != 1
           = clock_flicks + elapsed * 705600000 * rate / (1e9 * 1000)
             with elapsed = clamp(now - clock_host_ns, 0, 100 ms)   if playing
```

The 100 ms cap freezes the preview instead of running ahead if the clock writer stalls.

Liveness:

- `writer_state == 3` (closed): the engine shut down cleanly; release the mapping.
- `writer_state == 1` and `now − heartbeat_ns > 500 ms`: the engine is hung or dead; release
  the mapping and wait for Electron main's restart.
- `writer_state == 2` (retired): follow the successor (next section).

## Renegotiation and successor rings

The engine renders at the canvas pixel size (`view.set_preview_size`). A new size or format
that fits `slot_capacity` only bumps `config_gen` in block D. A larger one moves to a new ring:

1. The engine creates a new ring (new random name and epoch, slots at least 1.5x larger, so a
   window drag does not move on every step), with `write_index`, `frames_dropped`, the clock
   and block D carried over, so `seq` stays strictly increasing across rings.
2. It writes `successor_epoch` and `successor_name` into the old header, then
   `writer_state = 2` (Release).
3. It releases the old ring: the name disappears, but attached readers keep their mappings.

A reader that sees `writer_state == 2` (Acquire) opens `successor_name`, checks its
`ring_epoch == successor_epoch`, and switches. If the successor is already gone (two moves in
quick succession), it asks the engine for the current ring. The engine announces every move
(`Reconfigured::Moved`), so the run-file and handshake always carry the current name.

## Naming

`cwp-<pid: 8 lowercase hex>-<64 random bits from the OS CSPRNG: 16 lowercase hex>`, 29 bytes.
The pid lets the next engine start find a dead engine's rings; the random part makes the name
unguessable. OS form: `Local\cwp-…` on Windows (per-session namespace), `/cwp-…` for POSIX
`shm_open` (within macOS's 31-byte limit). The engine creates the ring exclusively, so a
pre-created (squatted) name fails and a fresh name is drawn. A launcher may choose the name
instead (`FramePublisher::create_named`), for example so Electron main can clean up after a
crash.

The engine announces `{name, epoch, layout_version, mapping_size}` (`RingAnnouncement`) in its
spawn handshake and in the run-file's `shm` field (WP-1I.01).

## Security posture

The ring holds rendered preview frames of the open project. The goal is that no other user
can read or tamper with them, and that the reader only accepts the engine's own object.
Processes of the same user at the same integrity level can already read the engine's memory
(debugging rights), so they are not a boundary any OS mechanism here can enforce.

| | Windows (implemented, tested) | Linux (implemented, compile-checked) | macOS (implemented, compile-checked) |
|---|---|---|---|
| Object | pagefile-backed file mapping `Local\<name>` | `/dev/shm/<name>` (tmpfs) | kernel POSIX shm object |
| Other users | DACL: protected, only the current user and OWNER RIGHTS, nothing inherited; no ACE for other users, groups or SYSTEM | mode `0600` → `0400`, owner verified with `fstat` | mode `0600` (→ `0400` where `fchmod` works), owner verified |
| Writing | only the engine's creation handle; every ACE grants `GENERIC_READ` only (map-read, query, read-control) | only the engine's mapping; later opens get `0400` read-only unless the same user changes the mode back | same as Linux where `fchmod` succeeds; otherwise the same user can open read-write |
| Loosening the ACL | `OWNER RIGHTS` ACE removes the owner's implicit `WRITE_DAC` | owner can `chmod` | owner can `chmod` |
| Sandboxed / low-integrity processes | Medium mandatory label, no-read-up + no-write-up: Low-integrity and AppContainer processes of the same user cannot open it | not separated (same uid) | not separated (same uid) |
| Name visibility | enumerable inside the session namespace | `ls /dev/shm` shows names to everyone; contents stay unreadable | not enumerable |
| Crash (`kill -9`) | kernel object freed with its last handle; nothing to reap | name persists; the next start's `reap_stale_rings()` unlinks rings whose pid is dead | name persists; `remove_stale_ring(name)` with names from stale run-files (no enumeration on macOS) |
| Reader checks | epoch | epoch; owner = euid, no group/other bits | same as Linux |

Achievable hardening not done in v1: Windows could restrict the DACL to the logon SID
instead of the user SID; Linux could use `memfd_create` and pass the descriptor over the
handshake socket (no name at all); macOS could pass a Mach memory entry. Each needs a
descriptor-passing channel to the preload that does not exist yet.

## Notes for the reader (WP-1H.03)

- Electron's V8 memory cage forbids external `ArrayBuffer`s over shared memory
  (`napi_no_external_buffers_allowed`), so the addon copies each frame once into a V8
  buffer (S1: about 0.7 ms for 8.3 MB). Reuse buffers per size rather than allocating per frame.
- Make every read method `&self`: an `AsyncTask` holding a shared borrow blocks `&mut`
  methods (S1 finding 8).
- Map read-only; never write to the ring, not even to "acknowledge" a frame.
- Run the present loop in the preload's isolated world with clock-scheduled choice; pass
  only stats across `contextBridge` (S1 findings 2 and 4).
- On Windows the ring of a dead engine lives as long as the addon maps it: release it on
  `engine-down`, on `writer_state == 3`, or on a stale heartbeat.
