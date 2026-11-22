//! Frame integrity helpers shared by the writer, the addon and (re-implemented in TS) the
//! renderer. Their purpose is to prove "zero torn frames" end to end (WP-0C.01 AC 2).

/// Every `CHECKSUM_WORD_STRIDE`-th little-endian `u32` word of the payload is hashed.
/// 61 is prime so the samples walk across columns instead of hitting the same x every row.
pub const CHECKSUM_WORD_STRIDE: usize = 61;

const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;

/// Word-wise FNV-1a over a sparse sample of the payload, finished with the payload length.
///
/// Cheap enough to recompute in JS for every 1080p frame (~34k words for RGBA8).
#[must_use]
pub fn sampled_fnv1a(payload: &[u8]) -> u32 {
    let words = payload.len() / 4;
    let mut h = FNV_OFFSET;
    let mut i = 0;
    while i < words {
        let o = i * 4;
        let w = u32::from_le_bytes([payload[o], payload[o + 1], payload[o + 2], payload[o + 3]]);
        h = (h ^ w).wrapping_mul(FNV_PRIME);
        i += CHECKSUM_WORD_STRIDE;
    }
    // Truncation is intended: the length only salts the hash.
    #[allow(clippy::cast_possible_truncation)]
    let len = payload.len() as u32;
    (h ^ len).wrapping_mul(FNV_PRIME)
}

/// Bytes stamped at the start of every plane-0 row.
pub const ROW_STAMP_LEN: usize = 8;

/// Write `seq` (u64 LE) into the first [`ROW_STAMP_LEN`] bytes of each of the `rows` rows of
/// plane 0. A torn copy shows up as rows carrying a different `seq` than the slot header.
pub fn stamp_rows(payload: &mut [u8], stride: usize, rows: usize, seq: u64) {
    let stamp = seq.to_le_bytes();
    for r in 0..rows {
        let o = r * stride;
        if o + ROW_STAMP_LEN > payload.len() {
            break;
        }
        payload[o..o + ROW_STAMP_LEN].copy_from_slice(&stamp);
    }
}

/// Count plane-0 rows whose stamp differs from `seq`.
#[must_use]
pub fn bad_row_stamps(payload: &[u8], stride: usize, rows: usize, seq: u64) -> usize {
    let stamp = seq.to_le_bytes();
    (0..rows)
        .filter(|r| {
            let o = r * stride;
            o + ROW_STAMP_LEN > payload.len() || payload[o..o + ROW_STAMP_LEN] != stamp
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_changes_with_content_and_length() {
        let a = vec![0u8; 4096];
        let mut b = a.clone();
        b[0] = 1;
        assert_ne!(sampled_fnv1a(&a), sampled_fnv1a(&b));
        assert_ne!(sampled_fnv1a(&a), sampled_fnv1a(&a[..4092]));
    }

    #[test]
    fn known_vector_matches_ts_implementation() {
        // Same vector is asserted in bridge/test/reader.test.mjs against the TS mirror.
        let payload: Vec<u8> = (0..1024u32).flat_map(|i| (i * 7).to_le_bytes()).collect();
        assert_eq!(sampled_fnv1a(&payload), 0x1b54_ff1d);
    }

    #[test]
    fn row_stamps_round_trip() {
        let mut p = vec![0u8; 64 * 10];
        stamp_rows(&mut p, 64, 10, 42);
        assert_eq!(bad_row_stamps(&p, 64, 10, 42), 0);
        p[64 * 3] ^= 1;
        assert_eq!(bad_row_stamps(&p, 64, 10, 42), 1);
    }
}
