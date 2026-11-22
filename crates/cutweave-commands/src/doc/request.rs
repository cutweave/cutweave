// SPDX-License-Identifier: GPL-3.0-or-later
//! The request envelope and its results: [`RequestId`], [`Submit`], [`Applied`], and the
//! transaction identifiers and summaries ([`TxId`], [`TxKind`], [`TxInfo`], [`TxClosed`]).

use std::{borrow::Cow, fmt, str::FromStr};

use cutweave_model::IdGen;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

use super::{
    clock::WallTime,
    identity::{Actor, ActorCtx, ConnId},
};
use crate::{error::Rev, patch::Patch};

/// The idempotency key of a request (arch §5.1, ADR-0015): a client-generated UUIDv7, or one
/// the engine generates for clients that omit it (CLI, MCP).
///
/// A repeat within the dedup window returns the original result without re-applying. Any
/// RFC 9562 UUID is accepted; text form is the usual lowercase `8-4-4-4-12` hex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub struct RequestId(u128);

/// Text that is not a UUID.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid request id `{0}`: expected a UUID like `0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f`")]
pub struct RequestIdError(pub String);

impl RequestId {
    /// The request id with these 128 bits.
    #[must_use]
    pub const fn from_u128(bits: u128) -> RequestId {
        RequestId(bits)
    }

    /// The 128 bits.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// A fresh UUIDv7(-shaped) id from `ids` (the engine's request-id stream, seeded under
    /// `--deterministic-ids`).
    pub fn generate(ids: &mut dyn IdGen) -> RequestId {
        RequestId(ids.next_u128())
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            v >> 96,
            (v >> 80) & 0xFFFF,
            (v >> 64) & 0xFFFF,
            (v >> 48) & 0xFFFF,
            v & 0xFFFF_FFFF_FFFF
        )
    }
}

impl fmt::Debug for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for RequestId {
    type Err = RequestIdError;

    fn from_str(s: &str) -> Result<RequestId, RequestIdError> {
        let err = || RequestIdError(s.to_owned());
        let bytes = s.as_bytes();
        let hex: String = match bytes.len() {
            36 if [8, 13, 18, 23].iter().all(|&i| bytes[i] == b'-') => {
                s.chars().filter(|&c| c != '-').collect()
            }
            32 => s.to_owned(),
            _ => return Err(err()),
        };
        if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
        u128::from_str_radix(&hex, 16)
            .map(RequestId)
            .map_err(|_| err())
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<RequestId, D::Error> {
        let s = Cow::<str>::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for RequestId {
    fn schema_name() -> Cow<'static, str> {
        "RequestId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "uuid",
            "description": "Idempotency key: a client-generated UUIDv7. A repeat within 10 minutes returns the original result."
        })
    }
}

/// A transaction: an implicit single-command one, or an open batch, gesture or proposal.
///
/// Allocated by the document actor in increasing order; `tx_<n>` on the wire.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub struct TxId(u64);

impl TxId {
    /// The transaction with this number.
    #[must_use]
    pub const fn new(n: u64) -> TxId {
        TxId(n)
    }

    /// The number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tx_{}", self.0)
    }
}

impl fmt::Debug for TxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Text that is not `tx_<n>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid transaction id `{0}`: expected `tx_<n>`")]
pub struct TxIdError(pub String);

impl FromStr for TxId {
    type Err = TxIdError;

    fn from_str(s: &str) -> Result<TxId, TxIdError> {
        s.strip_prefix("tx_")
            .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse().ok())
            .map(TxId)
            .ok_or_else(|| TxIdError(s.to_owned()))
    }
}

impl Serialize for TxId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TxId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TxId, D::Error> {
        let s = Cow::<str>::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for TxId {
    fn schema_name() -> Cow<'static, str> {
        "TxId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": "^tx_[0-9]+$",
            "description": "A transaction id."
        })
    }
}

/// What kind of open transaction (arch §5.2, ADR-0017).
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    /// A batch (`edit.begin_tx` / `edit.commit_tx`): its changes become visible only when it
    /// commits.
    Batch,
    /// A continuous gesture (dragging, scrubbing a slider): updates the document live, collapses
    /// into one undo step on commit, rolls back on cancel, auto-cancels when idle (WP-1B.09).
    Gesture,
    /// A copilot proposal (arch §14b): live like a gesture, no idle timeout.
    Proposal,
}

impl TxKind {
    /// Whether its uncommitted changes are visible in [`snapshot()`](super::DocState::snapshot)
    /// (gestures and proposals) or only after commit (batches).
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, TxKind::Gesture | TxKind::Proposal)
    }

    /// The wire name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            TxKind::Batch => "batch",
            TxKind::Gesture => "gesture",
            TxKind::Proposal => "proposal",
        }
    }
}

impl fmt::Display for TxKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A request for the document actor: run the registry entry `name` with `params` for `ctx`.
///
/// The envelope fields mirror the RPC `_meta` (arch §5.1, WP-1I.02): `request_id` (generated
/// when absent), `expected_rev` (optimistic concurrency) and `tx` (run inside an open
/// transaction of the caller's connection instead of an implicit single-command one).
#[derive(Clone, Debug)]
pub struct Submit {
    /// The authenticated caller (never taken from the request body).
    pub ctx: ActorCtx,
    /// Registry name (`clip.split`).
    pub name: String,
    /// JSON params (`null` reads as `{}`).
    pub params: Value,
    /// Idempotency key; the engine generates one when absent.
    pub request_id: Option<RequestId>,
    /// Fails with `Conflict{reason: stale_rev}` unless the document is at this revision.
    pub expected_rev: Option<Rev>,
    /// Run inside this open transaction (it must belong to `ctx`'s connection).
    pub tx: Option<TxId>,
}

impl Submit {
    /// A request with no envelope options.
    #[must_use]
    pub fn new(ctx: ActorCtx, name: impl Into<String>, params: Value) -> Submit {
        Submit {
            ctx,
            name: name.into(),
            params,
            request_id: None,
            expected_rev: None,
            tx: None,
        }
    }

    /// With an idempotency key.
    #[must_use]
    pub fn with_request_id(mut self, id: RequestId) -> Submit {
        self.request_id = Some(id);
        self
    }

    /// With an expected revision.
    #[must_use]
    pub fn with_expected_rev(mut self, rev: Rev) -> Submit {
        self.expected_rev = Some(rev);
        self
    }

    /// Inside an open transaction.
    #[must_use]
    pub fn in_tx(mut self, tx: TxId) -> Submit {
        self.tx = Some(tx);
        self
    }
}

/// The result of a request that succeeded.
///
/// What the dedup table stores and returns again for a repeated `request_id` (with
/// [`replayed`](Self::replayed) set), and what the journal persists with each transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    /// The request's idempotency key (generated when the client sent none).
    pub request_id: RequestId,
    /// The live document revision right after the request.
    pub rev: Rev,
    /// The last committed revision right after the request (`rev` when nothing is pending).
    pub committed_rev: Rev,
    /// The transaction it ran in (implicit or explicit); none for engine commands.
    pub tx_id: Option<TxId>,
    /// The command's output (a JSON object).
    pub output: Value,
    /// What the request changed: the committed patch of an implicit transaction, or the
    /// step's patch inside an open transaction. Empty for queries and no-ops.
    pub ops: Patch,
    /// Set when this is the stored result of an earlier request with the same `request_id`
    /// (nothing was re-applied).
    #[serde(skip)]
    pub replayed: bool,
}

/// An open transaction, as seen from outside the actor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxInfo {
    /// Its id.
    pub id: TxId,
    /// Batch, gesture or proposal.
    pub kind: TxKind,
    /// Human label (the undo entry's name, WP-1B.04).
    pub label: String,
    /// The connection that owns it.
    pub conn: ConnId,
    /// Who opened it.
    pub actor: Actor,
    /// The live revision when it was opened.
    pub opened_rev: Rev,
    /// When it was opened.
    pub opened_at: WallTime,
    /// When it last ran a command (gesture idle timeout, WP-1B.09).
    pub last_activity: WallTime,
    /// Commands run in it so far.
    pub steps: usize,
    /// Whether a commit by another transaction overlapped its changes, so it can only be
    /// cancelled.
    pub conflicted: bool,
}

/// How an open transaction ended.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
pub enum TxOutcome {
    /// Committed by its owner.
    Committed,
    /// Cancelled by its owner.
    Cancelled,
    /// Rolled back by the engine (its connection closed, WP-1B.09).
    RolledBack,
}

/// A transaction that was closed.
#[derive(Clone, Debug, PartialEq)]
pub struct TxClosed {
    /// Its id.
    pub tx_id: TxId,
    /// Its kind.
    pub kind: TxKind,
    /// How it ended.
    pub outcome: TxOutcome,
    /// The live revision afterwards.
    pub rev: Rev,
    /// The committed revision afterwards.
    pub committed_rev: Rev,
    /// The net committed change (coalesced, field-level); empty unless it committed changes.
    pub patch: Patch,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cutweave_model::SeededGen;

    use super::*;

    #[test]
    fn request_ids_round_trip_as_uuid_text() {
        let id = RequestId::from_u128(0x0190_f5c2_a1b2_7c3d_8e4f_5a6b_7c8d_9e0f);
        let text = "0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f";
        assert_eq!(id.to_string(), text);
        assert_eq!(text.parse::<RequestId>().unwrap(), id);
        assert_eq!(text.to_uppercase().parse::<RequestId>().unwrap(), id);
        assert_eq!(
            "0190f5c2a1b27c3d8e4f5a6b7c8d9e0f"
                .parse::<RequestId>()
                .unwrap(),
            id
        );
        let json = serde_json::to_value(id).unwrap();
        assert_eq!(json, serde_json::json!(text));
        assert_eq!(serde_json::from_value::<RequestId>(json).unwrap(), id);
        for bad in [
            "",
            "0190f5c2-a1b2-7c3d-8e4f",
            "0190f5c2a1b2-7c3d-8e4f-5a6b-7c8d9e0f",
            "0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0g",
            "+190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f",
        ] {
            assert!(bad.parse::<RequestId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn generated_request_ids_are_uuidv7_shaped_and_deterministic() {
        let a = RequestId::generate(&mut SeededGen::new(1));
        let b = RequestId::generate(&mut SeededGen::new(1));
        assert_eq!(a, b);
        let text = a.to_string();
        assert_eq!(text.as_bytes()[14], b'7', "{text}");
    }

    #[test]
    fn tx_ids_round_trip() {
        let id = TxId::new(42);
        assert_eq!(id.to_string(), "tx_42");
        assert_eq!("tx_42".parse::<TxId>().unwrap(), id);
        assert_eq!(
            serde_json::to_value(id).unwrap(),
            serde_json::json!("tx_42")
        );
        for bad in ["tx_", "tx_-1", "42", "tx_4a", "TX_4"] {
            assert!(bad.parse::<TxId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn tx_kinds() {
        assert!(!TxKind::Batch.is_live());
        assert!(TxKind::Gesture.is_live());
        assert!(TxKind::Proposal.is_live());
        assert_eq!(
            serde_json::to_value(TxKind::Gesture).unwrap(),
            serde_json::json!("gesture")
        );
    }
}
