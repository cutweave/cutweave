# Waveform pyramid format

Min/max peak pyramids of an audio stream, computed once by decoding the audio and stored as one file per `(content hash, audio stream)` in the OS cache directory (never in the project folder). Implemented in `cutweave_cache::{Waveform, WaveformCache}` (WP-1D.06); architecture: [§7](../01-architecture.md#7-media-pipeline-cutweave-media-cutweave-cache). The HTTP endpoint that serves it (`/media/{id}/waveform`, WP-1I.03) uses the *peak data* form below.

## Pyramid

- **Bins and levels.** Sample `n` of the stream (per channel, at the stream's native rate) falls in bin `n / 256` of level 0. Level 0 has `ceil(samples / 256)` bins; level N+1 has `ceil(bins(N) / 4)` bins and each is *exactly* the min of the mins and the max of the maxes of its four children (the last may have fewer). So a bin covers `256 × 4^lod` samples: 256, 1 024, 4 096, ... Levels continue until one bin covers the whole stream, at most 12.
- **Peaks** are `i16`: `round(clamp(sample, -1, 1) × 32767)`. A −6 dBFS sine reads ±16 384 (±1 LSB).
- **Tracks.** Track 0 is the mono mix: the mean of all channels per sample, then quantized (so a bin's min/max is that of the mixed signal). With `per_channel`, tracks `1..=channels` hold each source channel too (about `channels` times the size); a mono source never has them (its mix is its channel).
- **Time.** Bins are anchored at sample 0 of the stream, which is media time 0 (decoder timestamps decide where samples sit; a late-starting stream starts with silent bins).
- **Size.** 4 bytes per bin and track, level sizes sum to 4/3 of level 0: 1 h of 48 kHz stereo is 3.6 MB mix-only, 10.8 MB per-channel.

## File (`<hash>.<main|s<stream>>.cwwf`, little-endian)

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `CWWF` |
| 4 | 1 | format version (`1`) |
| 5 | 1 | flags: bit 0 = per-channel tracks present |
| 6 | 2 | channels of the source (`u16`) |
| 8 | 4 | sample rate in Hz (`u32`) |
| 12 | 4 | samples per bin of level 0 (`256`) |
| 16 | 4 | level factor (`4`) |
| 20 | 1 | level count |
| 21 | 1 | track count (`1`, or `channels + 1` with the per-channel flag) |
| 22 | 2 | reserved, zero |
| 24 | 8 | samples per channel (`u64`) |
| 32 | ... | for each track, for each level from 0: its bins as `min: i16, max: i16` |
| end - 8 | 8 | FNV-1a 64 checksum of every byte before it (`u64`) |

The per-level bin counts follow from the sample count, so the file size is exactly `32 + 4 × tracks × total bins + 8`; a reader rejects any other length, version, magic or checksum, and the cache treats the file as a miss (deleting it). Files are written to a temporary name and renamed into place.

The cache deletes nothing on its own: `WaveformCache::evict_lru(max_bytes)` removes the files read least recently (by modification time, refreshed on every read from disk).

## Peak data (query result and wire form)

`Waveform::peaks(lod, time_range, track) -> PeakData` returns the bins of one track at one level that overlap the time range (clamped to the audio; `first_bin` says where the slice starts). `PeakData::to_bytes` is the binary form the HTTP endpoint serves:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `CWPD` |
| 4 | 1 | version (`1`) |
| 5 | 1 | level (`lod`) |
| 6 | 1 | track kind: `0` mix, `1` channel |
| 7 | 1 | reserved |
| 8 | 2 | channel index (`u16`, 0 for the mix) |
| 10 | 2 | reserved |
| 12 | 4 | sample rate in Hz |
| 16 | 4 | samples per bin |
| 20 | 8 | `first_bin` (`u64`); the first sample covered is `first_bin × samples per bin` |
| 28 | 4 | bin count `n` |
| 32 | 4n | bins as `min: i16, max: i16` |

## Drawing

Pick the level with `Waveform::lod_for(samples_per_pixel)` (the coarsest whose bins are no wider than a pixel, so no peak is lost), query the visible time range, then draw one vertical line per pixel from the highest max to the lowest min of the bins that fall in that pixel. Scale `i16` by `1 / 32767`. `cutweave_cache::render_png(&PeakData, width, height, &WaveformStyle)` does exactly that into an RGBA PNG (the MCP `audio_waveform` tool, P2).
