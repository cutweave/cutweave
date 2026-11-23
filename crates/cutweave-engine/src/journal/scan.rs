// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading a project's journal back: find the segment chain that continues the saved document,
//! summarize it ([`RecoveryAvailable`]) and replay it through the model ([`replay`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cutweave_commands::{DedupRecord, JournaledCommit, Rev, TxId, WallTime};
use cutweave_model::Project;

use super::{
    format::{Decoded, Entry, SegmentHeader, StopReason, decode_segment},
    ids::IdState,
};

/// One segment file, decoded.
#[derive(Debug)]
pub(crate) struct SegmentScan {
    pub path: PathBuf,
    pub file_len: u64,
    pub header: Option<SegmentHeader>,
    pub entries: Vec<Decoded>,
    /// Where and why decoding stopped before the end of the file.
    pub stop: Option<(u64, StopReason)>,
}

impl SegmentScan {
    fn read(path: &Path) -> io::Result<SegmentScan> {
        let bytes = fs::read(path)?;
        let (entries, mut stop) = decode_segment(&bytes);
        let header = if let Some(Entry::Header(h)) = entries.first().map(|d| &d.entry) {
            Some(h.clone())
        } else {
            if stop.is_none() {
                stop = Some((0, StopReason::MissingHeader));
            }
            None
        };
        Ok(SegmentScan {
            path: path.to_path_buf(),
            file_len: bytes.len() as u64,
            header,
            entries,
            stop,
        })
    }
}

/// Every segment of a journal directory, and which of them continue the saved document.
#[derive(Debug, Default)]
pub(crate) struct DirScan {
    /// Segments with a header, oldest generation first.
    pub segments: Vec<SegmentScan>,
    /// Files that are not readable segments (no header).
    pub unreadable: Vec<PathBuf>,
    /// Index of the newest segment whose header names the saved document; the chain is it and
    /// every later segment.
    pub chain_start: Option<usize>,
}

impl DirScan {
    /// Scans `dir` (missing: empty) for the document whose bytes hash to `doc_sha256`.
    pub(crate) fn read(dir: &Path, doc_sha256: &str) -> io::Result<DirScan> {
        let mut scan = DirScan::default();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(scan),
            Err(e) => return Err(e),
        };
        for entry in entries {
            let path = entry?.path();
            let is_segment = path.is_file()
                && path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("jnl"));
            if !is_segment {
                continue;
            }
            let segment = SegmentScan::read(&path)?;
            if segment.header.is_some() {
                scan.segments.push(segment);
            } else {
                tracing::warn!(path = %path.display(), "journal file without a readable header; ignored");
                scan.unreadable.push(path);
            }
        }
        scan.segments.sort_by_key(|s| {
            s.header
                .as_ref()
                .map_or((0, Rev::ZERO), |h| (h.generation, h.base_rev))
        });
        scan.chain_start = scan.segments.iter().rposition(|s| {
            s.header
                .as_ref()
                .is_some_and(|h| h.doc_sha256 == doc_sha256)
        });
        Ok(scan)
    }

    /// The highest generation seen (0 when none).
    pub(crate) fn max_generation(&self) -> u64 {
        self.segments
            .iter()
            .filter_map(|s| s.header.as_ref().map(|h| h.generation))
            .max()
            .unwrap_or(0)
    }

    /// The chain's segments.
    pub(crate) fn chain(&self) -> &[SegmentScan] {
        self.chain_start
            .map_or(&[][..], |start| &self.segments[start..])
    }

    /// What replay would recover, without applying anything: `None` when the chain holds no
    /// transaction newer than the saved document.
    pub(crate) fn available(&self) -> Option<RecoveryAvailable> {
        let chain = self.chain();
        let base = chain.first()?.header.as_ref()?;
        let mut rev = base.base_rev;
        let mut records = 0;
        let mut last_ts = None;
        let mut damaged = false;
        'segments: for segment in chain {
            for decoded in &segment.entries {
                let Entry::Commit(c) = &decoded.entry else {
                    continue;
                };
                let c = &c.commit;
                if c.rev <= rev {
                    continue;
                }
                if c.prev_committed_rev != rev {
                    damaged = true;
                    break 'segments;
                }
                rev = c.rev;
                records += 1;
                last_ts = Some(c.at);
            }
            if segment.stop.is_some() {
                damaged = true;
                break;
            }
        }
        Some(RecoveryAvailable {
            records,
            base_rev: base.base_rev,
            last_rev: rev,
            last_ts: last_ts?,
            damaged,
        })
    }
}

/// Unsaved work found in the journal when a project is opened (arch §17): the recovery UI
/// offers to restore or discard it (`project.recover` / `project.discard_recovery`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryAvailable {
    /// Transactions committed after the last save that replay would restore.
    pub records: usize,
    /// The committed revision of the saved document.
    pub base_rev: Rev,
    /// The committed revision after the last of them.
    pub last_rev: Rev,
    /// When the last of them was committed.
    pub last_ts: WallTime,
    /// The journal ends with a torn or corrupt record (a crash mid-write; never an
    /// acknowledged transaction) or a gap; replay stops there.
    pub damaged: bool,
}

/// Where replay stopped early, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayStop {
    /// The segment.
    pub segment: PathBuf,
    /// The offset of the record that was not replayed.
    pub offset: u64,
    /// Why.
    pub reason: StopReason,
}

/// What a replay did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayReport {
    /// Transactions replayed.
    pub applied: usize,
    /// Copies of transactions already replayed (carried over by a rotation) that were skipped.
    pub duplicates: usize,
    /// The first record not replayed: a torn tail after a crash (expected; never an
    /// acknowledged transaction), or corruption. Everything after it is ignored.
    pub stopped: Option<ReplayStop>,
}

/// Where appending continues after a replay.
#[derive(Debug)]
pub(crate) struct ResumePoint {
    /// Index into [`DirScan::segments`].
    pub segment: usize,
    /// Offset just past the last valid entry.
    pub offset: u64,
    /// Each transaction kept in that segment: committed revision and frame offset.
    pub revs: Vec<(Rev, u64)>,
}

/// The result of replaying a chain.
#[derive(Debug)]
pub(crate) struct Replayed {
    pub project: Project,
    pub rev: Rev,
    pub commits: Vec<JournaledCommit>,
    pub requests: Vec<DedupRecord>,
    pub ids: Option<IdState>,
    pub seed: Option<u64>,
    pub max_tx: u64,
    pub report: ReplayReport,
    pub resume: ResumePoint,
}

/// Replays the chain of `scan` onto `base` (the saved document): every transaction in commit
/// order, validated (checksum and decoding happened in the scan; here: revision continuity and
/// the patch's preimages against the document), stopping at the first record that fails.
/// Without `apply`, stops before the first transaction (discarding the unsaved work). `None`
/// when no segment continues `base`.
pub(crate) fn replay(scan: &DirScan, base: &Project, apply: bool) -> Option<Replayed> {
    let replayed = replay_inner(scan, base, apply, false)?;
    if replayed.report.applied == 0 {
        return Some(replayed);
    }
    // Patches with matching preimages reproduce the committed documents exactly, and the
    // engine never commits one that breaks an invariant; check once, and only when something
    // is off, find the first record that introduced it.
    let before = base.violations();
    if replayed
        .project
        .violations()
        .iter()
        .all(|v| before.contains(v))
    {
        Some(replayed)
    } else {
        replay_inner(scan, base, apply, true)
    }
}

// One pass over the chain with its validation steps inline; splitting it would scatter the
// resume-point bookkeeping.
#[allow(clippy::too_many_lines)]
fn replay_inner(scan: &DirScan, base: &Project, apply: bool, check_each: bool) -> Option<Replayed> {
    let start = scan.chain_start?;
    let header = scan.segments[start].header.as_ref()?;
    let before = if check_each {
        base.violations()
    } else {
        Vec::new()
    };
    let mut out = Replayed {
        project: base.clone(),
        rev: header.base_rev,
        commits: Vec::new(),
        requests: Vec::new(),
        ids: header.ids,
        seed: header.seed,
        max_tx: 0,
        report: ReplayReport::default(),
        resume: ResumePoint {
            segment: start,
            offset: 0,
            revs: Vec::new(),
        },
    };
    'segments: for (index, segment) in scan.segments.iter().enumerate().skip(start) {
        if index != start {
            out.resume = ResumePoint {
                segment: index,
                offset: 0,
                revs: Vec::new(),
            };
        }
        for decoded in &segment.entries {
            let stop = |reason| ReplayStop {
                segment: segment.path.clone(),
                offset: decoded.start,
                reason,
            };
            match &decoded.entry {
                Entry::Header(h) => {
                    // Positions at the segment's start (rotation time) cover every
                    // transaction before it, including carried ones.
                    if h.ids.is_some() {
                        out.ids = h.ids;
                    }
                }
                Entry::Carry(carry) => {
                    for r in &carry.requests {
                        out.max_tx = out.max_tx.max(r.applied.tx_id.map_or(0, TxId::get));
                    }
                    out.requests.extend(carry.requests.iter().cloned());
                }
                Entry::Commit(entry) => {
                    let c = &entry.commit;
                    if c.rev <= out.rev {
                        out.report.duplicates += 1;
                        out.resume.revs.push((c.rev, decoded.start));
                        out.resume.offset = decoded.end;
                        continue;
                    }
                    if !apply {
                        break 'segments;
                    }
                    if c.prev_committed_rev != out.rev {
                        out.report.stopped = Some(stop(StopReason::Gap {
                            expected: out.rev,
                            found: c.prev_committed_rev,
                        }));
                        break 'segments;
                    }
                    let next = match c.patch.apply(&out.project) {
                        Ok(doc) => doc,
                        Err(e) => {
                            out.report.stopped =
                                Some(stop(StopReason::PatchMismatch(e.to_string())));
                            break 'segments;
                        }
                    };
                    if check_each
                        && let Some(v) = next.violations().into_iter().find(|v| !before.contains(v))
                    {
                        out.report.stopped = Some(stop(StopReason::Invariant(v.to_string())));
                        break 'segments;
                    }
                    out.project = next;
                    out.rev = c.rev;
                    out.max_tx = out.max_tx.max(c.tx_id.get());
                    out.requests
                        .extend(c.steps.iter().map(|s| s.record.clone()));
                    if entry.ids.is_some() {
                        out.ids = entry.ids;
                    }
                    out.commits.push(c.clone());
                    out.report.applied += 1;
                    out.resume.revs.push((c.rev, decoded.start));
                }
            }
            out.resume.offset = decoded.end;
        }
        if let Some((offset, reason)) = &segment.stop {
            out.report.stopped = Some(ReplayStop {
                segment: segment.path.clone(),
                offset: *offset,
                reason: reason.clone(),
            });
            break;
        }
    }
    Some(out)
}
