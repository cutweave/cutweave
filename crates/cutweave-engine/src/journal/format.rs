// SPDX-License-Identifier: GPL-3.0-or-later
//! The on-disk journal format: segment files of checksummed, length-prefixed JSON entries.
//!
//! ```text
//! segment := MAGIC frame*                      MAGIC = "CWJRNL" 0x00 0x01
//! frame   := len:u32le payload[len] crc:u32le   crc = CRC-32(len bytes ++ payload)
//! payload := UTF-8 JSON object, {"type": "header" | "carry" | "commit" | "checkpoint", ...}
//! ```
//!
//! The first frame of a segment is its [`SegmentHeader`]; a [`Carry`] may follow; every other
//! frame is one committed transaction ([`CommitEntry`]) or a change of the checkpoints
//! ([`CheckpointEntry`], WP-2I.03). The checksum covers the length too, so a torn or
//! bit-flipped length is detected like a torn payload. Entries of an unknown type (written by a
//! newer engine) are skipped.

use cutweave_commands::{
    CheckpointInfo, CheckpointRecord, CommitRecord, DedupRecord, JournaledCheckpoint,
    JournaledCommit, Patch, Rev, WallTime,
};
use serde::{Deserialize, Serialize};

use super::{JournalError, crc::crc32, ids::IdState};

/// The first bytes of every segment file.
pub(crate) const MAGIC: &[u8; 8] = b"CWJRNL\x00\x01";

/// The current format version (in every [`SegmentHeader`]). Version 2 (WP-2I.03) adds
/// checkpoint entries and the carried checkpoints; it reads version 1 segments unchanged.
pub const FORMAT_VERSION: u32 = 2;

/// The first entry of a segment: what it builds on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentHeader {
    /// [`FORMAT_VERSION`] when written.
    pub format: u32,
    /// Increases with every segment of a project; orders segments that share a base revision.
    pub generation: u64,
    /// The committed revision of the document this segment's first transaction builds on.
    pub base_rev: Rev,
    /// SHA-256 (hex) of the saved `.cutweave` document bytes at `base_rev`: how recovery finds
    /// the segment that continues the document on disk.
    pub doc_sha256: String,
    /// When the segment was started.
    pub created_at: WallTime,
    /// The `--deterministic-ids` seed of the session that wrote it, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// The deterministic id streams' positions when the segment started (with `seed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdState>,
}

/// What a new segment carries over from before the save it starts with: the dedup records of
/// transactions folded into the saved document that are still inside the dedup window (arch
/// §5.1), and the checkpoints made before the save (WP-2I.03), each as the patch that turns the
/// saved document into the checkpointed one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Carry {
    /// The remembered requests.
    pub requests: Vec<DedupRecord>,
    /// The checkpoints made before the save.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CarriedCheckpoint>,
    /// The highest checkpoint number issued so far (numbers are never reused).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub checkpoints_issued: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `skip_serializing_if` passes a reference
const fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Carry {
    /// Whether there is nothing to carry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty() && self.checkpoints.is_empty() && self.checkpoints_issued == 0
    }
}

/// A checkpoint carried over a save (WP-2I.03).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarriedCheckpoint {
    /// Its metadata.
    #[serde(flatten)]
    pub info: CheckpointInfo,
    /// The ops that turn the segment's base document (the saved one) into the checkpointed
    /// document.
    pub patch: Patch,
}

/// One committed transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommitEntry {
    /// The transaction as the doc actor committed it (redacted params, dedup records, patch).
    pub commit: JournaledCommit,
    /// The deterministic id generators' positions right after it (`--deterministic-ids` only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdState>,
}

/// One change of the checkpoints (WP-2I.03): the checkpoint made (its metadata; replay rebuilds
/// its document at `rev`) and the ones removed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckpointEntry {
    /// The change as the doc actor made it.
    pub checkpoint: JournaledCheckpoint,
    /// The deterministic id generators' positions right after it (`--deterministic-ids`
    /// only): its request id may have come from them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<IdState>,
}

/// A decoded entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Entry {
    Header(SegmentHeader),
    Carry(Carry),
    Commit(Box<CommitEntry>),
    Checkpoint(Box<CheckpointEntry>),
    /// An entry type this version does not know (a newer engine wrote it): skipped.
    #[serde(other)]
    Unknown,
}

/// The borrowed form of a commit entry, serialized on the hot path without copying the record.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommitRef<'a> {
    Commit {
        commit: &'a CommitRecord<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ids: Option<IdState>,
    },
}

/// The borrowed form of a checkpoint entry.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CheckpointRef<'a> {
    Checkpoint {
        checkpoint: &'a CheckpointRecord<'a>,
        #[serde(skip_serializing_if = "Option::is_none")]
        ids: Option<IdState>,
    },
}

/// Appends one frame holding `payload` to `out`.
pub(crate) fn push_frame(out: &mut Vec<u8>, payload: &[u8]) -> Result<(), JournalError> {
    let len = u32::try_from(payload.len()).map_err(|_| JournalError::TooLarge(payload.len()))?;
    let len_bytes = len.to_le_bytes();
    out.reserve(payload.len() + 8);
    out.extend_from_slice(&len_bytes);
    out.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&[&len_bytes, payload]).to_le_bytes());
    Ok(())
}

/// One frame holding `entry`.
pub(crate) fn entry_frame(entry: &Entry) -> Result<Vec<u8>, JournalError> {
    let payload = serde_json::to_vec(entry).map_err(JournalError::Encode)?;
    let mut out = Vec::new();
    push_frame(&mut out, &payload)?;
    Ok(out)
}

/// One frame holding the commit `record`.
pub(crate) fn commit_frame(
    record: &CommitRecord<'_>,
    ids: Option<IdState>,
) -> Result<Vec<u8>, JournalError> {
    let payload = serde_json::to_vec(&CommitRef::Commit {
        commit: record,
        ids,
    })
    .map_err(JournalError::Encode)?;
    let mut out = Vec::new();
    push_frame(&mut out, &payload)?;
    Ok(out)
}

/// One frame holding the checkpoint change `record`.
pub(crate) fn checkpoint_frame(
    record: &CheckpointRecord<'_>,
    ids: Option<IdState>,
) -> Result<Vec<u8>, JournalError> {
    let payload = serde_json::to_vec(&CheckpointRef::Checkpoint {
        checkpoint: record,
        ids,
    })
    .map_err(JournalError::Encode)?;
    let mut out = Vec::new();
    push_frame(&mut out, &payload)?;
    Ok(out)
}

/// Why reading a segment stopped before its end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// The file does not start with the journal magic.
    BadMagic,
    /// The file ends inside a frame (a write cut short by a crash).
    Truncated,
    /// A frame's checksum does not match (corruption).
    Checksum,
    /// A frame's checksum matches but its payload is not a journal entry.
    Malformed(String),
    /// The segment has no header.
    MissingHeader,
    /// A transaction does not continue the revision before it.
    Gap {
        /// The committed revision it should build on.
        expected: Rev,
        /// The revision it builds on.
        found: Rev,
    },
    /// A transaction's patch does not apply to the document (its preimages differ).
    PatchMismatch(String),
    /// Replaying it would leave the document violating a model invariant.
    Invariant(String),
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopReason::BadMagic => f.write_str("not a journal segment (bad magic)"),
            StopReason::Truncated => f.write_str("torn record at the end (cut short by a crash)"),
            StopReason::Checksum => f.write_str("record checksum mismatch (corrupt)"),
            StopReason::Malformed(e) => write!(f, "record is not a journal entry: {e}"),
            StopReason::MissingHeader => f.write_str("segment has no header"),
            StopReason::Gap { expected, found } => write!(
                f,
                "transaction builds on revision {found}, expected {expected}"
            ),
            StopReason::PatchMismatch(e) => write!(f, "transaction does not apply: {e}"),
            StopReason::Invariant(e) => write!(f, "transaction breaks a model invariant: {e}"),
        }
    }
}

/// A decoded frame and where it is in the file.
#[derive(Clone, Debug)]
pub(crate) struct Decoded {
    /// Offset of the frame's first byte.
    pub start: u64,
    /// Offset just past it.
    pub end: u64,
    pub entry: Entry,
}

/// Every intact entry of a segment's bytes, in order, and why decoding stopped early (if it
/// did). Never panics, whatever the bytes.
pub(crate) fn decode_segment(bytes: &[u8]) -> (Vec<Decoded>, Option<(u64, StopReason)>) {
    let mut out = Vec::new();
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        let reason = if MAGIC.starts_with(bytes) {
            StopReason::Truncated
        } else {
            StopReason::BadMagic
        };
        return (out, Some((0, reason)));
    }
    let mut pos = MAGIC.len();
    while pos < bytes.len() {
        let start = pos as u64;
        let Some(len_bytes) = bytes.get(pos..pos + 4) else {
            return (out, Some((start, StopReason::Truncated)));
        };
        let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        // A garbage length past the end of the file reads as a torn record (checked
        // arithmetic: no overflow whatever the bytes).
        let Some(payload_end) = usize::try_from(len)
            .ok()
            .and_then(|len| (pos + 4).checked_add(len))
        else {
            return (out, Some((start, StopReason::Truncated)));
        };
        let Some(payload) = bytes.get(pos + 4..payload_end) else {
            return (out, Some((start, StopReason::Truncated)));
        };
        let Some(crc_bytes) = payload_end
            .checked_add(4)
            .and_then(|end| bytes.get(payload_end..end))
        else {
            return (out, Some((start, StopReason::Truncated)));
        };
        let crc = u32::from_le_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
        if crc != crc32(&[len_bytes, payload]) {
            return (out, Some((start, StopReason::Checksum)));
        }
        let entry = match serde_json::from_slice::<Entry>(payload) {
            Ok(entry) => entry,
            Err(e) => return (out, Some((start, StopReason::Malformed(e.to_string())))),
        };
        pos = payload_end + 4;
        out.push(Decoded {
            start,
            end: pos as u64,
            entry,
        });
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_commands::{Rev, WallTime};
    use proptest::prelude::*;

    use super::*;

    fn segment() -> Vec<u8> {
        let header = SegmentHeader {
            format: FORMAT_VERSION,
            generation: 3,
            base_rev: Rev(7),
            doc_sha256: "ab".repeat(32),
            created_at: WallTime::from_millis(5),
            seed: Some(42),
            ids: Some(IdState {
                entity: 2,
                request: 1,
            }),
        };
        let mut bytes = MAGIC.to_vec();
        bytes.extend(entry_frame(&Entry::Header(header)).unwrap());
        bytes.extend(entry_frame(&Entry::Carry(Carry::default())).unwrap());
        bytes
    }

    #[test]
    fn frames_round_trip() {
        let bytes = segment();
        let (entries, stop) = decode_segment(&bytes);
        assert_eq!(stop, None);
        assert_eq!(entries.len(), 2);
        assert!(
            matches!(&entries[0].entry, Entry::Header(h) if h.base_rev == Rev(7) && h.seed == Some(42))
        );
        assert_eq!(entries[1].end, bytes.len() as u64);
    }

    #[test]
    fn entries_of_unknown_types_are_skipped_not_corruption() {
        let mut bytes = segment();
        push_frame(&mut bytes, br#"{"type":"from_the_future","x":1}"#).unwrap();
        let (entries, stop) = decode_segment(&bytes);
        assert_eq!(stop, None);
        assert_eq!(entries[2].entry, Entry::Unknown);
    }

    #[test]
    fn carries_without_checkpoints_read_as_version_1_wrote_them() {
        let carry: Carry = serde_json::from_str(r#"{"requests": []}"#).unwrap();
        assert!(carry.is_empty());
        let json = serde_json::to_value(Entry::Carry(Carry::default())).unwrap();
        assert_eq!(json, serde_json::json!({"type": "carry", "requests": []}));
    }

    #[test]
    fn magic_is_checked() {
        assert_eq!(decode_segment(b"").1, Some((0, StopReason::Truncated)));
        assert_eq!(decode_segment(b"CWJR").1, Some((0, StopReason::Truncated)));
        assert_eq!(
            decode_segment(b"{\"type\":1}").1,
            Some((0, StopReason::BadMagic))
        );
    }

    #[test]
    fn every_flipped_bit_is_detected() {
        let bytes = segment();
        for i in MAGIC.len()..bytes.len() {
            for bit in 0..8 {
                let mut bad = bytes.clone();
                bad[i] ^= 1 << bit;
                let (entries, stop) = decode_segment(&bad);
                assert!(stop.is_some(), "flip at byte {i} bit {bit} went unnoticed");
                assert!(entries.len() < 2);
            }
        }
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(tail in prop::collection::vec(any::<u8>(), 0..512)) {
            let mut bytes = segment();
            bytes.extend(tail);
            let (entries, _) = decode_segment(&bytes);
            prop_assert!(entries.len() >= 2);
        }
    }
}
