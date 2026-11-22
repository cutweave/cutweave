// SPDX-License-Identifier: GPL-3.0-or-later
//! `request_id` deduplication (arch §5.1, ADR-0015).
//!
//! The actor remembers the [`Applied`] result of every successful request for
//! [`DEDUP_WINDOW`] of wall-clock time. A repeat of the same `request_id` within the window, by
//! the same actor with the same command and params, returns that result without re-applying
//! anything; with a different actor or payload it is a `Conflict{reason: request_id_reused}`.
//! Failed requests are not remembered: they changed nothing, so a retry simply runs again.
//!
//! Every committed transaction carries its [`DedupRecord`]s to the journal (WP-1C.02), and a
//! restarted engine seeds the table from them, so the window survives restarts.

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    time::Duration,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Number, Value};

use super::{
    clock::WallTime,
    identity::Actor,
    request::{Applied, RequestId},
};

/// How long a `request_id` is remembered: 10 minutes of wall-clock time (ADR-0015).
pub const DEDUP_WINDOW: Duration = Duration::from_mins(10);

/// Default bound on remembered requests. The window normally bounds the table; this caps it
/// under extreme load (or a wall clock stepping backwards) by forgetting the oldest first.
pub const DEDUP_CAPACITY: usize = 65_536;

/// A stable 64-bit digest of who sent a request and what it asked for: actor, command name and
/// the redacted params (keys sorted, numbers by value), so a resend that re-serializes the same
/// object differently still matches. FNV-1a, so it is the same in every build and can be
/// persisted.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(u64);

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    fn tag(&mut self, tag: u8) {
        self.bytes(&[tag]);
    }

    fn len(&mut self, n: usize) {
        self.bytes(&u64::try_from(n).unwrap_or(u64::MAX).to_le_bytes());
    }

    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.bytes(s.as_bytes());
    }

    fn number(&mut self, n: &Number) {
        // Integral values hash the same whatever their JSON spelling (`1`, `1.0`).
        if let Some(i) = n.as_i64() {
            self.tag(b'i');
            self.bytes(&i.to_le_bytes());
        } else if let Some(u) = n.as_u64() {
            self.tag(b'u');
            self.bytes(&u.to_le_bytes());
        } else {
            let f = n.as_f64().unwrap_or(f64::NAN);
            #[allow(clippy::cast_possible_truncation, clippy::float_cmp)]
            if f.fract() == 0.0 && f.abs() < 9.0e18 {
                self.tag(b'i');
                self.bytes(&(f as i64).to_le_bytes());
            } else {
                self.tag(b'f');
                self.bytes(&f.to_bits().to_le_bytes());
            }
        }
    }

    fn value(&mut self, v: &Value) {
        match v {
            Value::Null => self.tag(0),
            Value::Bool(b) => {
                self.tag(1);
                self.tag(u8::from(*b));
            }
            Value::Number(n) => {
                self.tag(2);
                self.number(n);
            }
            Value::String(s) => {
                self.tag(3);
                self.str(s);
            }
            Value::Array(items) => {
                self.tag(4);
                self.len(items.len());
                for item in items {
                    self.value(item);
                }
            }
            Value::Object(map) => {
                self.tag(5);
                self.len(map.len());
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                for (key, value) in entries {
                    self.str(key);
                    self.value(value);
                }
            }
        }
    }
}

impl Fingerprint {
    /// The fingerprint of `actor` calling `name` with (already redacted) `params`. `null`
    /// params hash like `{}` (the registry reads them the same).
    #[must_use]
    pub fn of(actor: &Actor, name: &str, params: &Value) -> Fingerprint {
        let mut h = Fnv(FNV_OFFSET);
        h.str(&actor.to_string());
        h.str(name);
        match params {
            Value::Null => h.value(&Value::Object(serde_json::Map::new())),
            other => h.value(other),
        }
        Fingerprint(h.0)
    }

    /// The raw digest.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

// Hex text: a JSON number would lose bits above 2^53 in JavaScript tooling.
impl Serialize for Fingerprint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fingerprint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Fingerprint, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        if s.len() != 16 {
            return Err(de::Error::custom("a fingerprint is 16 hex digits"));
        }
        u64::from_str_radix(&s, 16)
            .map(Fingerprint)
            .map_err(de::Error::custom)
    }
}

/// A remembered request: what the journal persists per request and what a restarted engine
/// seeds the dedup table with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DedupRecord {
    /// Who sent what ([`Fingerprint::of`]).
    pub fingerprint: Fingerprint,
    /// When it was applied (wall clock; the window is measured from here).
    pub at: WallTime,
    /// Its result.
    pub applied: Applied,
}

impl DedupRecord {
    /// The request's id.
    #[must_use]
    pub fn request_id(&self) -> RequestId {
        self.applied.request_id
    }

    /// Whether a repeat at `now` falls within `window` of this record.
    #[must_use]
    pub fn is_live(&self, now: WallTime, window: Duration) -> bool {
        now.saturating_since(self.at) <= window
    }
}

/// The table of remembered requests.
#[derive(Debug)]
pub(crate) struct DedupTable {
    window: Duration,
    capacity: usize,
    entries: BTreeMap<RequestId, DedupRecord>,
    /// Insertion order, for expiry and the capacity bound. May hold stale pairs for ids that
    /// were re-inserted; those are skipped by comparing `at`.
    order: VecDeque<(WallTime, RequestId)>,
}

impl DedupTable {
    pub(crate) fn new(window: Duration, capacity: usize) -> DedupTable {
        DedupTable {
            window,
            capacity: capacity.max(1),
            entries: BTreeMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn window(&self) -> Duration {
        self.window
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The record for `id` if it is within the window at `now`. Forgets expired records.
    pub(crate) fn lookup(&mut self, id: RequestId, now: WallTime) -> Option<&DedupRecord> {
        self.expire(now);
        let live = self.entries.get(&id)?.is_live(now, self.window);
        if live {
            self.entries.get(&id)
        } else {
            self.entries.remove(&id);
            None
        }
    }

    /// Remembers `record` (replacing an expired one with the same id).
    pub(crate) fn insert(&mut self, record: DedupRecord) {
        let key = (record.at, record.request_id());
        self.entries.insert(key.1, record);
        self.order.push_back(key);
        while self.entries.len() > self.capacity {
            let Some((at, id)) = self.order.pop_front() else {
                break;
            };
            self.remove_if_at(id, at);
        }
        // Keep `order` from growing without bound when ids are re-inserted.
        if self.order.len() > 2 * self.capacity {
            let entries = &self.entries;
            self.order
                .retain(|(at, id)| entries.get(id).is_some_and(|r| r.at == *at));
        }
    }

    fn remove_if_at(&mut self, id: RequestId, at: WallTime) {
        if self.entries.get(&id).is_some_and(|r| r.at == at) {
            self.entries.remove(&id);
        }
    }

    /// Forgets records older than the window, oldest first.
    fn expire(&mut self, now: WallTime) {
        while let Some(&(at, id)) = self.order.front() {
            if now.saturating_since(at) <= self.window {
                break;
            }
            self.order.pop_front();
            self.remove_if_at(id, at);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::*;
    use crate::{error::Rev, patch::Patch};

    fn record(id: u128, at_ms: u64) -> DedupRecord {
        DedupRecord {
            fingerprint: Fingerprint(7),
            at: WallTime::from_millis(at_ms),
            applied: Applied {
                request_id: RequestId::from_u128(id),
                rev: Rev(1),
                committed_rev: Rev(1),
                tx_id: None,
                output: json!({}),
                ops: Patch::new(),
                replayed: false,
            },
        }
    }

    #[test]
    fn fingerprints_ignore_key_order_and_number_spelling() {
        let user = Actor::User;
        let a = Fingerprint::of(
            &user,
            "clip.split",
            &serde_json::from_str(r#"{"item":"itm_1","at":30}"#).unwrap(),
        );
        let b = Fingerprint::of(
            &user,
            "clip.split",
            &serde_json::from_str(r#"{"at":30.0,"item":"itm_1"}"#).unwrap(),
        );
        assert_eq!(a, b);
        let other_value = Fingerprint::of(&user, "clip.split", &json!({"item": "itm_1", "at": 31}));
        let other_name = Fingerprint::of(&user, "clip.trim", &json!({"item": "itm_1", "at": 30}));
        let other_actor = Fingerprint::of(
            &Actor::System,
            "clip.split",
            &json!({"item": "itm_1", "at": 30}),
        );
        assert_ne!(a, other_value);
        assert_ne!(a, other_name);
        assert_ne!(a, other_actor);
        assert_eq!(
            Fingerprint::of(&user, "project.noop", &Value::Null),
            Fingerprint::of(&user, "project.noop", &json!({}))
        );
        // Nesting is not ambiguous: ["ab"] vs ["a","b"].
        assert_ne!(
            Fingerprint::of(&user, "x.y", &json!(["ab"])),
            Fingerprint::of(&user, "x.y", &json!(["a", "b"]))
        );
    }

    #[test]
    fn fingerprints_are_stable_across_builds() {
        // Persisted in the journal: this value must never change.
        let f = Fingerprint::of(&Actor::User, "project.rename", &json!({"name": "A"}));
        assert_eq!(f.to_string().len(), 16);
        let back: Fingerprint = serde_json::from_value(serde_json::to_value(f).unwrap()).unwrap();
        assert_eq!(back, f);
        assert_eq!(f.to_string(), "e174998d5c0af61b");
    }

    #[test]
    fn window_is_inclusive_and_expires() {
        let mut t = DedupTable::new(DEDUP_WINDOW, 16);
        t.insert(record(1, 0));
        let nine = WallTime::from_millis(9 * 60_000);
        assert!(t.lookup(RequestId::from_u128(1), nine).is_some());
        let ten = WallTime::from_millis(10 * 60_000);
        assert!(t.lookup(RequestId::from_u128(1), ten).is_some());
        let eleven = WallTime::from_millis(11 * 60_000);
        assert!(t.lookup(RequestId::from_u128(1), eleven).is_none());
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn capacity_forgets_the_oldest() {
        let mut t = DedupTable::new(DEDUP_WINDOW, 3);
        for id in 1..=5 {
            t.insert(record(id, u64::try_from(id).unwrap()));
        }
        assert_eq!(t.len(), 3);
        let now = WallTime::from_millis(10);
        assert!(t.lookup(RequestId::from_u128(1), now).is_none());
        assert!(t.lookup(RequestId::from_u128(2), now).is_none());
        assert!(t.lookup(RequestId::from_u128(5), now).is_some());
    }

    #[test]
    fn reinserting_an_id_keeps_the_newest_record() {
        let mut t = DedupTable::new(Duration::from_secs(60), 8);
        t.insert(record(1, 0));
        t.insert(record(1, 50_000));
        // The first pair in `order` is stale; expiring it must not drop the new record.
        let now = WallTime::from_millis(100_000);
        assert!(t.lookup(RequestId::from_u128(1), now).is_some());
        for i in 0..100 {
            t.insert(record(2, i));
        }
        assert!(t.order.len() <= 2 * 8 + 1);
    }
}
