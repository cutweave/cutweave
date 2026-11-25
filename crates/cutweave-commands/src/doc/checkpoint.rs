// SPDX-License-Identifier: GPL-3.0-or-later
//! Checkpoints (WP-1B.04, WP-2I.03; arch §5.2, §17): restore points of the committed document,
//! and how they reach the journal and come back after a restart.
//!
//! # In memory
//!
//! `edit.checkpoint` keeps an O(1) snapshot of the committed document (the persistent
//! collections share everything that does not change later) with its metadata, in a registry of
//! at most [`MAX_CHECKPOINTS`], at most [`MAX_NAMED_CHECKPOINTS`] of them named; the least
//! recently used (made or restored) go first, and reusing a name moves it to the new checkpoint.
//! Ids (`cp_<n>`) are never reused, also across a restart. `edit.delete_checkpoint` removes one.
//! Nothing is created automatically, except one per `cutweave_execute{checkpoint: true}` call.
//!
//! # In the journal
//!
//! Every change of the registry reaches the [`CommitSink`](super::CommitSink) as a
//! [`CheckpointRecord`] before it is acknowledged: the checkpoint made (its metadata and the
//! committed revision it captured, not the document) and the ids that went away (deleted,
//! trimmed by the limits, or displaced by a reused name), with the request for deduplication. On
//! recovery the journal replays these markers through a [`CheckpointLedger`]: a checkpoint's
//! document is the replayed document at its revision. A save starts a new journal segment over
//! the saved document, and the checkpoints made before it are carried into the new segment as
//! patches from the saved document (the journal's part, in `cutweave-engine`), so they survive a
//! crash after any number of saves, and closing and reopening the project.
//! [`DocConfig::with_recovered_checkpoints`](super::DocConfig::with_recovered_checkpoints) hands
//! the ledger to the new document state. A checkpoint the journal cannot rebuild is dropped:
//! restoring it is `NotFound`.
//!
//! # Restoring after a restart
//!
//! A restore refuses, unless forced, to revert changes by other actors or changes it cannot
//! attribute. After a restart the history holds only the transactions replayed from the journal
//! (those since the last save), so restoring a checkpoint made before the last save reverts
//! changes of unknown authorship and needs `force: true`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use cutweave_model::Project;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::{clock::WallTime, identity::Actor, state::JournalStep};
use crate::error::Rev;

/// Most checkpoints kept (least recently used unnamed ones go first).
pub const MAX_CHECKPOINTS: usize = 50;
/// Most named checkpoints kept (least recently used go first).
pub const MAX_NAMED_CHECKPOINTS: usize = 20;
/// Longest checkpoint name, in characters.
pub const MAX_CHECKPOINT_NAME_CHARS: usize = 64;
/// Longest checkpoint note, in characters.
pub const MAX_CHECKPOINT_NOTE_CHARS: usize = 1_000;

/// A checkpoint (`cp_<n>` on the wire), see `edit.checkpoint`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub struct CheckpointId(u64);

impl CheckpointId {
    /// The checkpoint with this number.
    #[must_use]
    pub const fn new(n: u64) -> CheckpointId {
        CheckpointId(n)
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Parses `cp_<n>`.
    #[must_use]
    pub fn parse(s: &str) -> Option<CheckpointId> {
        s.strip_prefix("cp_")
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
            .map(CheckpointId)
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cp_{}", self.0)
    }
}

impl fmt::Debug for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Serialize for CheckpointId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CheckpointId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<CheckpointId, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        CheckpointId::parse(&s).ok_or_else(|| {
            de::Error::custom(format!("invalid checkpoint id `{s}`: expected `cp_<n>`"))
        })
    }
}

impl JsonSchema for CheckpointId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CheckpointId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^cp_[0-9]+$",
            "description": "A checkpoint id."
        })
    }
}

/// A checkpoint, as `edit.checkpoint` and `edit.list_checkpoints` report it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
pub struct CheckpointInfo {
    /// Its id.
    pub checkpoint: CheckpointId,
    /// Its name, if any.
    pub name: Option<String>,
    /// A free-form note.
    pub note: Option<String>,
    /// The committed revision it captured.
    pub rev: Rev,
    /// Who made it.
    pub actor: Actor,
    /// When.
    pub at: WallTime,
}

/// The document a checkpoint captured, with its metadata.
#[derive(Clone, Debug)]
pub(crate) struct Checkpoint {
    pub(crate) info: CheckpointInfo,
    pub(crate) doc: Project,
    /// The first history record after it (0: it predates the history, as after a restart).
    pub(crate) first_seq: u64,
    last_used: u64,
}

/// Removes and returns the least recently used checkpoint of `kept` (sorted by use) that is
/// named or not.
fn take_lru<'a>(kept: &mut Vec<&'a Checkpoint>, named: bool) -> Option<&'a Checkpoint> {
    kept.iter()
        .position(|c| c.info.name.is_some() == named)
        .map(|i| kept.remove(i))
}

/// The checkpoint registry: O(1) snapshots of the committed document thanks to the persistent
/// collections (only what changes later costs memory).
#[derive(Debug, Default)]
pub(crate) struct Checkpoints {
    /// The highest checkpoint number issued (ids are never reused).
    issued: u64,
    tick: u64,
    map: BTreeMap<CheckpointId, Checkpoint>,
}

impl Checkpoints {
    /// The registry rebuilt from the journal's ledger; `first_seq` maps a checkpoint's
    /// revision to the first recovered history record after it.
    pub(crate) fn recovered(
        ledger: CheckpointLedger,
        first_seq: impl Fn(Rev) -> u64,
    ) -> Checkpoints {
        let mut out = Checkpoints {
            issued: ledger.issued,
            ..Checkpoints::default()
        };
        for (id, entry) in ledger.entries {
            out.tick += 1;
            let first_seq = first_seq(entry.info.rev);
            out.issued = out.issued.max(id.get());
            out.map.insert(
                id,
                Checkpoint {
                    info: entry.info,
                    doc: entry.doc,
                    first_seq,
                    last_used: out.tick,
                },
            );
        }
        out
    }

    /// The highest checkpoint number issued.
    pub(crate) fn issued(&self) -> u64 {
        self.issued
    }

    /// The id the next checkpoint gets.
    pub(crate) fn next_id(&self) -> CheckpointId {
        CheckpointId(self.issued + 1)
    }

    /// What making a checkpoint named `name` (or unnamed) removes: the one holding that name,
    /// then the least recently used ones beyond the limits (named ones beyond
    /// [`MAX_NAMED_CHECKPOINTS`] first, then unnamed ones, then named ones beyond
    /// [`MAX_CHECKPOINTS`]). The new checkpoint itself always stays.
    pub(crate) fn displaced(&self, name: Option<&str>) -> Vec<CheckpointId> {
        let (mut out, mut kept): (Vec<&Checkpoint>, Vec<&Checkpoint>) = self
            .map
            .values()
            .partition(|c| name.is_some() && c.info.name.as_deref() == name);
        kept.sort_by_key(|c| c.last_used);
        let mut named =
            kept.iter().filter(|c| c.info.name.is_some()).count() + usize::from(name.is_some());
        while named > MAX_NAMED_CHECKPOINTS {
            let Some(c) = take_lru(&mut kept, true) else {
                break;
            };
            out.push(c);
            named -= 1;
        }
        while kept.len() + 1 > MAX_CHECKPOINTS {
            let Some(c) = take_lru(&mut kept, false).or_else(|| take_lru(&mut kept, true)) else {
                break;
            };
            out.push(c);
        }
        let mut ids: Vec<CheckpointId> = out.into_iter().map(|c| c.info.checkpoint).collect();
        ids.sort_unstable();
        ids
    }

    /// Adds a checkpoint, after removing `displaced` (from [`displaced`](Self::displaced)).
    pub(crate) fn insert(
        &mut self,
        info: CheckpointInfo,
        doc: Project,
        first_seq: u64,
        displaced: &[CheckpointId],
    ) {
        for id in displaced {
            self.map.remove(id);
        }
        self.issued = self.issued.max(info.checkpoint.get());
        self.tick += 1;
        self.map.insert(
            info.checkpoint,
            Checkpoint {
                info,
                doc,
                first_seq,
                last_used: self.tick,
            },
        );
    }

    /// The checkpoint with this id (`cp_<n>`) or name.
    pub(crate) fn resolve(&self, key: &str) -> Option<CheckpointId> {
        CheckpointId::parse(key)
            .filter(|id| self.map.contains_key(id))
            .or_else(|| {
                self.map
                    .values()
                    .find(|c| c.info.name.as_deref() == Some(key))
                    .map(|c| c.info.checkpoint)
            })
    }

    /// The checkpoint with this id or name, marked as used.
    pub(crate) fn find(&mut self, key: &str) -> Option<&Checkpoint> {
        let id = self.resolve(key)?;
        self.tick += 1;
        let tick = self.tick;
        let c = self.map.get_mut(&id)?;
        c.last_used = tick;
        Some(c)
    }

    /// Removes a checkpoint.
    pub(crate) fn remove(&mut self, id: CheckpointId) -> Option<Checkpoint> {
        self.map.remove(&id)
    }

    /// Every checkpoint, oldest first.
    pub(crate) fn list(&self) -> Vec<CheckpointInfo> {
        self.map.values().map(|c| c.info.clone()).collect()
    }

    /// The first history record after each checkpoint is not newer than `next_seq`
    /// (diagnostics).
    pub(crate) fn verify(&self, next_seq: u64) -> Result<(), String> {
        if self.map.len() > MAX_CHECKPOINTS {
            return Err(format!("{} checkpoints kept", self.map.len()));
        }
        let named = self.map.values().filter(|c| c.info.name.is_some()).count();
        if named > MAX_NAMED_CHECKPOINTS {
            return Err(format!("{named} named checkpoints kept"));
        }
        for c in self.map.values() {
            if c.first_seq > next_seq {
                return Err(format!(
                    "{}: first record {} is in the future",
                    c.info.checkpoint, c.first_seq
                ));
            }
            if c.info.checkpoint.get() > self.issued {
                return Err(format!("{} was never issued", c.info.checkpoint));
            }
        }
        Ok(())
    }
}

/// A change of the checkpoints, handed to the [`CommitSink`](super::CommitSink) before it is
/// acknowledged (WP-2I.03). Serialized (without the document) as the journal's marker.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct CheckpointRecord<'a> {
    /// The committed revision when it happened; a new checkpoint captured this revision.
    pub rev: Rev,
    /// The checkpoint made, if any.
    pub created: Option<&'a CheckpointInfo>,
    /// The checkpoints that went away: deleted, trimmed by the limits, or displaced by a reused
    /// name.
    pub removed: &'a [CheckpointId],
    /// The highest checkpoint number issued so far (numbers are never reused).
    pub issued: u64,
    /// The `edit.checkpoint` / `edit.delete_checkpoint` request with its dedup record, when it
    /// came through [`DocState::submit`](super::DocState::submit).
    pub steps: &'a [JournalStep],
    /// The committed document at `rev` (what `created` captured). Not serialized: recovery
    /// rebuilds it by replaying the journal to `rev`.
    #[serde(skip)]
    pub doc: &'a Project,
}

impl CheckpointRecord<'_> {
    /// The owned form (what the journal reads back).
    #[must_use]
    pub fn to_owned(&self) -> JournaledCheckpoint {
        JournaledCheckpoint {
            rev: self.rev,
            created: self.created.cloned(),
            removed: self.removed.to_vec(),
            issued: self.issued,
            steps: self.steps.to_vec(),
        }
    }
}

/// A [`CheckpointRecord`] read back from the journal (same JSON form).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournaledCheckpoint {
    /// The committed revision when it happened.
    pub rev: Rev,
    /// The checkpoint made, if any.
    #[serde(default)]
    pub created: Option<CheckpointInfo>,
    /// The checkpoints that went away.
    #[serde(default)]
    pub removed: Vec<CheckpointId>,
    /// The highest checkpoint number issued so far.
    #[serde(default)]
    pub issued: u64,
    /// The request.
    #[serde(default)]
    pub steps: Vec<JournalStep>,
}

/// One checkpoint of a [`CheckpointLedger`]: its metadata and document.
#[derive(Clone, Debug)]
pub struct LedgerEntry {
    /// Its metadata.
    pub info: CheckpointInfo,
    /// The committed document it captured.
    pub doc: Project,
}

/// The checkpoints as the journal knows them: what each [`CheckpointRecord`] made and removed
/// (WP-2I.03). The journal keeps one to carry the checkpoints across a save, and rebuilds one
/// on recovery for [`DocConfig::with_recovered_checkpoints`](super::DocConfig::with_recovered_checkpoints).
#[derive(Clone, Debug, Default)]
pub struct CheckpointLedger {
    issued: u64,
    entries: BTreeMap<CheckpointId, LedgerEntry>,
    /// Every id made or carried so far: a copy of an earlier marker (journal segments repeat
    /// the transactions committed while a save was prepared) changes nothing.
    seen: BTreeSet<CheckpointId>,
}

impl CheckpointLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> CheckpointLedger {
        CheckpointLedger::default()
    }

    /// A live record (its document is at hand).
    pub fn record(&mut self, record: &CheckpointRecord<'_>) {
        self.apply(
            record.created,
            record.removed,
            record.issued,
            Some(record.doc),
        );
    }

    /// A record read back from the journal; `doc` is the replayed document at `record.rev`,
    /// when replay has it (a checkpoint made without one is dropped). Returns whether the
    /// record was new: a copy of a record replayed before changes nothing.
    pub fn replay(&mut self, record: &JournaledCheckpoint, doc: Option<&Project>) -> bool {
        self.apply(record.created.as_ref(), &record.removed, record.issued, doc)
    }

    fn apply(
        &mut self,
        created: Option<&CheckpointInfo>,
        removed: &[CheckpointId],
        issued: u64,
        doc: Option<&Project>,
    ) -> bool {
        let fresh = created.is_none_or(|c| !self.seen.contains(&c.checkpoint));
        if !fresh {
            return false;
        }
        for id in removed {
            self.entries.remove(id);
        }
        self.issued = self.issued.max(issued);
        if let Some(info) = created {
            self.seen.insert(info.checkpoint);
            self.issued = self.issued.max(info.checkpoint.get());
            if let Some(doc) = doc {
                self.entries.insert(
                    info.checkpoint,
                    LedgerEntry {
                        info: info.clone(),
                        doc: doc.clone(),
                    },
                );
            } else {
                tracing::warn!(
                    checkpoint = %info.checkpoint,
                    rev = %info.rev,
                    "a checkpoint's document cannot be rebuilt from the journal; it is dropped"
                );
            }
        }
        true
    }

    /// A checkpoint carried over a save, its document rebuilt from the saved one.
    pub fn carry(&mut self, info: CheckpointInfo, doc: Project) {
        self.seen.insert(info.checkpoint);
        self.issued = self.issued.max(info.checkpoint.get());
        self.entries
            .insert(info.checkpoint, LedgerEntry { info, doc });
    }

    /// Raises the highest number issued (from a segment's carry).
    pub fn issue(&mut self, issued: u64) {
        self.issued = self.issued.max(issued);
    }

    /// The highest checkpoint number issued.
    #[must_use]
    pub fn issued(&self) -> u64 {
        self.issued
    }

    /// The checkpoint with this id.
    #[must_use]
    pub fn get(&self, id: CheckpointId) -> Option<&LedgerEntry> {
        self.entries.get(&id)
    }

    /// Every checkpoint, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &LedgerEntry> {
        self.entries.values()
    }

    /// How many checkpoints there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn info(n: u64, name: Option<&str>, rev: u64) -> CheckpointInfo {
        CheckpointInfo {
            checkpoint: CheckpointId::new(n),
            name: name.map(str::to_owned),
            note: None,
            rev: Rev(rev),
            actor: Actor::User,
            at: WallTime::UNIX_EPOCH,
        }
    }

    fn mk(c: &mut Checkpoints, name: Option<&str>) -> (CheckpointId, Vec<CheckpointId>) {
        let displaced = c.displaced(name);
        let id = c.next_id();
        c.insert(info(id.get(), name, 1), Project::new("x"), 1, &displaced);
        (id, displaced)
    }

    #[test]
    fn ids_parse() {
        assert_eq!(CheckpointId::parse("cp_12"), Some(CheckpointId::new(12)));
        for bad in ["cp_", "cp_x", "12", "CP_1"] {
            assert_eq!(CheckpointId::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn checkpoint_names_move_and_lru_trims() {
        let mut c = Checkpoints::default();
        let (first, _) = mk(&mut c, Some("a"));
        let (second, displaced) = mk(&mut c, Some("a"));
        assert_ne!(first, second);
        assert_eq!(displaced, [first]);
        assert_eq!(c.find("a").unwrap().info.checkpoint, second);
        assert!(c.find(&first.to_string()).is_none());
        for _ in 0..MAX_CHECKPOINTS + 5 {
            mk(&mut c, None);
        }
        assert_eq!(c.list().len(), MAX_CHECKPOINTS);
        // The named one survives the unnamed churn.
        assert!(c.find("a").is_some());
        for i in 0..MAX_NAMED_CHECKPOINTS + 3 {
            mk(&mut c, Some(&format!("n{i}")));
        }
        let named = c.list().iter().filter(|i| i.name.is_some()).count();
        assert_eq!(named, MAX_NAMED_CHECKPOINTS);
        c.verify(1).unwrap();
        // Ids are never reused.
        let ids: Vec<u64> = c.list().iter().map(|i| i.checkpoint.get()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(
            c.next_id().get(),
            2 + (MAX_CHECKPOINTS as u64 + 5) + (MAX_NAMED_CHECKPOINTS as u64 + 3) + 1
        );
    }

    #[test]
    fn a_full_registry_displaces_the_least_recently_used() {
        let mut c = Checkpoints::default();
        let (oldest, _) = mk(&mut c, None);
        let (second, _) = mk(&mut c, None);
        for _ in 2..MAX_CHECKPOINTS {
            mk(&mut c, None);
        }
        // Restoring the oldest makes it recently used: the second goes instead.
        c.find(&oldest.to_string()).unwrap();
        let (_, displaced) = mk(&mut c, None);
        assert_eq!(displaced, [second]);
        assert!(c.resolve(&oldest.to_string()).is_some());
    }

    #[test]
    fn the_ledger_skips_copies_and_keeps_numbers_rising() {
        let doc = Project::new("d");
        let mut ledger = CheckpointLedger::new();
        let made = JournaledCheckpoint {
            rev: Rev(3),
            created: Some(info(1, Some("a"), 3)),
            removed: Vec::new(),
            issued: 1,
            steps: Vec::new(),
        };
        let deleted = JournaledCheckpoint {
            rev: Rev(4),
            created: None,
            removed: vec![CheckpointId::new(1)],
            issued: 1,
            steps: Vec::new(),
        };
        assert!(ledger.replay(&made, Some(&doc)));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.replay(&deleted, None));
        assert!(ledger.is_empty());
        // A later segment repeats both: the deleted checkpoint stays deleted.
        assert!(!ledger.replay(&made, Some(&doc)));
        ledger.replay(&deleted, None);
        assert!(ledger.is_empty());
        assert_eq!(ledger.issued(), 1);
        // A marker whose document replay does not have is dropped, but its number is spent.
        let lost = JournaledCheckpoint {
            created: Some(info(2, None, 9)),
            issued: 2,
            ..made.clone()
        };
        ledger.replay(&lost, None);
        assert!(ledger.is_empty());
        assert_eq!(ledger.issued(), 2);
        // Carried ones count as seen.
        ledger.carry(info(5, None, 1), doc.clone());
        assert_eq!(ledger.issued(), 5);
        let copy = JournaledCheckpoint {
            created: Some(info(5, None, 1)),
            ..made
        };
        assert!(!ledger.replay(&copy, Some(&doc)));
        assert_eq!(ledger.len(), 1);
    }
}
