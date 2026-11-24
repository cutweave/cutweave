// SPDX-License-Identifier: GPL-3.0-or-later
//! Rate limiting for library log lines (FFmpeg's `av_log`, routed to `tracing`).
//!
//! FFmpeg repeats some lines hundreds of times per second: a seek into an open GOP logs
//! `mmco: unref short failure` for every dropped leading picture (S3 finding F2.4). The filter
//! keeps the first [`BURST`] occurrences of each distinct line, then only the 2^k-th occurrence
//! with its count, so a line repeated `n` times costs `O(BURST + log2 n)` log records. Lines
//! are compared without the context pointer FFmpeg prints (`[h264 @ 000001F2A3B4C5D0]`), so
//! repeats from different decoder instances count together.
//!
//! Deterministic (no clock) and bounded: at most [`CAPACITY`] distinct lines are tracked; the
//! least recently seen one is forgotten first.

/// Occurrences of a line always logged before rate limiting starts.
pub(crate) const BURST: u64 = 3;
/// Distinct lines tracked.
pub(crate) const CAPACITY: usize = 64;

/// What to do with one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Log it as is.
    Emit,
    /// Log it with "repeated `n` times" (this is the `n`-th occurrence).
    EmitRepeated(u64),
    /// Drop it.
    Suppress,
}

#[derive(Debug)]
struct Seen {
    key: String,
    count: u64,
    last: u64,
}

/// Per-line occurrence counts.
#[derive(Debug)]
pub(crate) struct RepeatFilter {
    seen: Vec<Seen>,
    tick: u64,
}

impl RepeatFilter {
    /// An empty filter (usable in a `static`).
    pub(crate) const fn new() -> RepeatFilter {
        RepeatFilter {
            seen: Vec::new(),
            tick: 0,
        }
    }

    /// Counts `line` and decides whether to log it.
    pub(crate) fn check(&mut self, line: &str) -> Verdict {
        self.tick += 1;
        let key = normalize(line);
        let count = if let Some(s) = self.seen.iter_mut().find(|s| s.key == key) {
            s.count += 1;
            s.last = self.tick;
            s.count
        } else {
            if self.seen.len() >= CAPACITY
                && let Some(oldest) = self
                    .seen
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, s)| s.last)
                    .map(|(i, _)| i)
            {
                self.seen.swap_remove(oldest);
            }
            self.seen.push(Seen {
                key,
                count: 1,
                last: self.tick,
            });
            1
        };
        if count <= BURST {
            Verdict::Emit
        } else if count.is_power_of_two() {
            Verdict::EmitRepeated(count)
        } else {
            Verdict::Suppress
        }
    }
}

/// `line` with FFmpeg context addresses (`@ 0x7ff…`, `@ 000001F2…`) replaced by `@ _`.
pub(crate) fn normalize(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("@ ") {
        out.push_str(&rest[..at + 2]);
        let after = &rest[at + 2..];
        let hex = after.strip_prefix("0x").unwrap_or(after);
        let digits = hex.len()
            - hex
                .trim_start_matches(|c: char| c.is_ascii_hexdigit())
                .len();
        if digits >= 4 {
            out.push('_');
            rest = &hex[digits..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_pointers_are_ignored() {
        assert_eq!(
            normalize("[h264 @ 000001F2A3B4C5D0] mmco: unref short failure"),
            "[h264 @ _] mmco: unref short failure"
        );
        assert_eq!(
            normalize("[hevc @ 0x7f3a2c001200] Could not find ref with POC 12"),
            "[hevc @ _] Could not find ref with POC 12"
        );
        assert_eq!(normalize("mail me @ home"), "mail me @ home");
        assert_eq!(normalize("trailing @ "), "trailing @ ");
    }

    #[test]
    fn a_flood_costs_a_logarithmic_number_of_lines() {
        let mut f = RepeatFilter::new();
        let mut emitted = Vec::new();
        for i in 1..=1000_u64 {
            // Two decoder instances logging the same line.
            let line = if i % 2 == 0 {
                "[h264 @ 0000020000000001] mmco: unref short failure"
            } else {
                "[h264 @ 0000020000000002] mmco: unref short failure"
            };
            match f.check(line) {
                Verdict::Emit => emitted.push(i),
                Verdict::EmitRepeated(n) => {
                    assert_eq!(n, i);
                    emitted.push(i);
                }
                Verdict::Suppress => {}
            }
        }
        assert_eq!(emitted, [1, 2, 3, 4, 8, 16, 32, 64, 128, 256, 512]);
    }

    #[test]
    fn distinct_lines_are_counted_separately() {
        let mut f = RepeatFilter::new();
        for _ in 0..BURST {
            assert_eq!(f.check("a"), Verdict::Emit);
        }
        assert_eq!(f.check("b"), Verdict::Emit);
        assert_eq!(f.check("a"), Verdict::EmitRepeated(4));
        assert_eq!(f.check("a"), Verdict::Suppress);
    }

    #[test]
    fn the_table_is_bounded_and_forgets_the_stalest_line() {
        let mut f = RepeatFilter::new();
        for _ in 0..BURST {
            f.check("old");
        }
        for i in 0..CAPACITY {
            f.check(&format!("line {i}"));
        }
        assert_eq!(f.seen.len(), CAPACITY);
        // "old" was evicted, so it starts over.
        assert_eq!(f.check("old"), Verdict::Emit);
        assert_eq!(f.seen.len(), CAPACITY);
    }
}
