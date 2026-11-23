// SPDX-License-Identifier: GPL-3.0-or-later
//! CRC-32 (IEEE 802.3: reflected, polynomial `0xEDB88320`, the zlib/PNG checksum) for journal
//! frames. Table-driven; a few lines, so the journal needs no checksum dependency.

const POLY: u32 = 0xEDB8_8320;

const TABLE: [u32; 256] = table();

const fn table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut i = 0;
    while i < 256 {
        #[allow(clippy::cast_possible_truncation)]
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 { POLY ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

/// The CRC-32 of the concatenation of `parts`.
pub(crate) fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = !0_u32;
    for part in parts {
        for &b in *part {
            c = TABLE[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test]
    fn standard_check_values() {
        assert_eq!(crc32(&[b""]), 0);
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(
            crc32(&[b"The quick brown fox jumps over the lazy dog"]),
            0x414F_A339
        );
    }

    #[test]
    fn parts_concatenate() {
        assert_eq!(crc32(&[b"1234", b"", b"56789"]), crc32(&[b"123456789"]));
    }
}
