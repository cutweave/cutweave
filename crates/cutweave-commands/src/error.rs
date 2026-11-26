// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed command errors (arch §5.1, ADR-0015): a machine `code` with structured fields, plus a
//! human `message`. Agents rely on both, so the wire form always carries both.

use std::{borrow::Cow, fmt};

use cutweave_model::EntityKind;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{doc::Actor, meta::Scope};

/// Document revision: increases by one per committed transaction (WP-1B.03).
///
/// A JSON integer on the wire and a TS `number`.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    specta::Type,
)]
#[serde(transparent)]
pub struct Rev(#[specta(type = specta_typescript::Number)] pub u64);

impl Rev {
    /// The revision of a freshly opened document.
    pub const ZERO: Rev = Rev(0);

    /// The next revision (saturating).
    #[must_use]
    pub const fn next(self) -> Rev {
        Rev(self.0.saturating_add(1))
    }
}

impl fmt::Display for Rev {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a [`CmdError::NotFound`] or [`CmdError::Locked`] refers to.
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
    specta::Type,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResourceKind {
    /// A media asset (`med_…`).
    Media,
    /// A sequence (`seq_…`).
    Sequence,
    /// A track (`trk_…`).
    Track,
    /// A timeline item (`itm_…`).
    Item,
    /// A linked group (`grp_…`).
    Group,
    /// A marker (`mrk_…`).
    Marker,
    /// A folder of the media bin (`fld_…`, ADR-0020).
    Folder,
    /// A registry entry: an unknown command or query name.
    Command,
    /// An open transaction (WP-1B.04).
    Transaction,
    /// A checkpoint (`cp_…` or its name, `edit.checkpoint`, WP-1B.04).
    Checkpoint,
    /// A background job (`job_…`, WP-1D.07).
    Job,
    /// A keyframe, addressed by item, property and time (`keyframe.*`, WP-2C.03).
    Keyframe,
}

impl ResourceKind {
    /// Lowercase name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ResourceKind::Media => "media",
            ResourceKind::Sequence => "sequence",
            ResourceKind::Track => "track",
            ResourceKind::Item => "item",
            ResourceKind::Group => "group",
            ResourceKind::Marker => "marker",
            ResourceKind::Folder => "folder",
            ResourceKind::Command => "command",
            ResourceKind::Transaction => "transaction",
            ResourceKind::Checkpoint => "checkpoint",
            ResourceKind::Job => "job",
            ResourceKind::Keyframe => "keyframe",
        }
    }
}

impl From<EntityKind> for ResourceKind {
    fn from(kind: EntityKind) -> ResourceKind {
        match kind {
            EntityKind::Media => ResourceKind::Media,
            EntityKind::Sequence => ResourceKind::Sequence,
            EntityKind::Track => ResourceKind::Track,
            EntityKind::Item => ResourceKind::Item,
            EntityKind::Group => ResourceKind::Group,
            EntityKind::Marker => ResourceKind::Marker,
            EntityKind::Folder => ResourceKind::Folder,
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Machine-readable reason of a [`CmdError::Conflict`].
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, specta::Type,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConflictReason {
    /// The request's `expected_rev` is not the current revision (WP-1B.03).
    StaleRev,
    /// A `request_id` was reused with a different payload (WP-1B.03).
    RequestIdReused,
    /// An undo or redo target was changed by a later transaction, so its recorded patch no
    /// longer applies or would leave the document invalid; or a checkpoint restore would revert
    /// other actors' changes without `force` (per-actor undo, WP-1B.04). The detail names the
    /// paths and the transactions and actors that changed them.
    HistoryTargetModified,
    /// The item is soft-locked by another actor's open gesture or proposal transaction
    /// (arch §5.2, ADR-0017); retry after `retry_after_ms`.
    ItemInGesture,
    /// Opening or closing a project would drop the open document's unsaved changes, and the
    /// caller did not say to (`discard_unsaved: true` / `save: false`, ADR-0029). The detail
    /// names the project.
    UnsavedChanges,
}

impl ConflictReason {
    /// The wire name (`stale_rev`, …).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            ConflictReason::StaleRev => "stale_rev",
            ConflictReason::RequestIdReused => "request_id_reused",
            ConflictReason::HistoryTargetModified => "history_target_modified",
            ConflictReason::ItemInGesture => "item_in_gesture",
            ConflictReason::UnsavedChanges => "unsaved_changes",
        }
    }
}

impl fmt::Display for ConflictReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Every error a command, query or engine command can return (arch §5.1).
///
/// Each variant has a machine [`code`](Self::code) and a human [`message`](Self::message). On
/// the wire it is an object tagged by `code`, with the variant's fields and `message`:
///
/// ```json
/// {"code": "invalid", "path": "/at", "msg": "…", "message": "invalid value at `/at`: …"}
/// ```
///
/// `Invalid.path` is a JSON Pointer into the **request params**, so agents can fix the exact
/// argument; `""` means the params object itself.
#[derive(Clone, Debug, PartialEq, thiserror::Error, specta::Type)]
#[specta(type = wire::CmdErrorWire)]
#[non_exhaustive]
pub enum CmdError {
    /// An ID (or command name) does not exist.
    #[error("{kind} `{id}` not found")]
    NotFound {
        /// What kind of thing.
        kind: ResourceKind,
        /// The missing ID or name.
        id: String,
    },
    /// The target is locked: a locked track or item (arch §4.2), or an open transaction owned
    /// by another connection (WP-1B.09; then `by` and `tx` are set). An item soft-locked by
    /// another actor's open gesture or proposal is a `Conflict{reason: item_in_gesture}`
    /// instead (arch §5.1).
    #[error("{kind} `{id}` is locked{}", locked_by(by.as_deref(), tx.as_deref()))]
    Locked {
        /// What kind of thing is locked.
        kind: ResourceKind,
        /// Its ID.
        id: String,
        /// The actor holding a soft lock (`agent:<label>`, `user`, …).
        by: Option<String>,
        /// The transaction holding a soft lock.
        tx: Option<String>,
    },
    /// The request conflicts with the current state (stale revision, reused `request_id`,
    /// undo target modified, item in another actor's gesture).
    #[error("conflict ({reason}): {detail}{}", at_rev(*current_rev))]
    Conflict {
        /// Machine reason.
        reason: ConflictReason,
        /// The document revision when the error was raised (filled in by the engine).
        current_rev: Option<Rev>,
        /// When retrying may succeed (soft locks): the time left until the holding gesture's
        /// idle deadline; none for a proposal, which waits for a person (WP-2A.12).
        retry_after_ms: Option<u64>,
        /// The actor holding the soft lock (`user`, `agent:<label>`, …) of an `item_in_gesture`
        /// conflict (WP-2A.12, ADR-0023); none for the other reasons.
        holder: Option<Actor>,
        /// Human detail.
        detail: String,
    },
    /// A param failed validation (schema or semantic check). `path` is a JSON Pointer into the
    /// request params (`""` for the params object itself).
    #[error("{}", invalid_message(path, msg))]
    Invalid {
        /// JSON Pointer (RFC 6901) into the params.
        path: String,
        /// What is wrong.
        msg: String,
    },
    /// The caller's credential lacks the scope (e.g. an agent calling a UI-only command,
    /// arch §6, ADR-0016). Enforced by the registry, so batches cannot bypass it.
    #[error("forbidden: {msg}")]
    Forbidden {
        /// The scope the command needs.
        scope: Scope,
        /// What to do instead.
        msg: String,
    },
    /// A valid request this version cannot do (e.g. a track kind schema v1 does not edit).
    #[error("unsupported: {msg}")]
    Unsupported {
        /// What is unsupported.
        msg: String,
    },
    /// A bug or a panic in the engine. Never carries secrets.
    #[error("internal error: {msg}")]
    Internal {
        /// What failed.
        msg: String,
    },
}

fn locked_by(by: Option<&str>, tx: Option<&str>) -> String {
    match (by, tx) {
        (Some(by), Some(tx)) => format!(" by {by} (transaction {tx})"),
        (Some(by), None) => format!(" by {by}"),
        (None, Some(tx)) => format!(" by transaction {tx}"),
        (None, None) => String::new(),
    }
}

fn at_rev(rev: Option<Rev>) -> String {
    rev.map_or_else(String::new, |rev| format!(" (current rev {rev})"))
}

fn invalid_message(path: &str, msg: &str) -> String {
    if path.is_empty() {
        format!("invalid params: {msg}")
    } else {
        format!("invalid value at `{path}`: {msg}")
    }
}

impl CmdError {
    /// `NotFound` for an entity ID.
    pub fn not_found(kind: impl Into<ResourceKind>, id: impl fmt::Display) -> CmdError {
        CmdError::NotFound {
            kind: kind.into(),
            id: id.to_string(),
        }
    }

    /// `Locked` for a locked entity (no soft-lock holder).
    pub fn locked(kind: impl Into<ResourceKind>, id: impl fmt::Display) -> CmdError {
        CmdError::Locked {
            kind: kind.into(),
            id: id.to_string(),
            by: None,
            tx: None,
        }
    }

    /// `Conflict` without a revision (the engine fills it in with [`with_rev`](Self::with_rev)).
    pub fn conflict(reason: ConflictReason, detail: impl Into<String>) -> CmdError {
        CmdError::Conflict {
            reason,
            current_rev: None,
            retry_after_ms: None,
            holder: None,
            detail: detail.into(),
        }
    }

    /// `Invalid` at a JSON Pointer into the params.
    pub fn invalid(path: impl Into<String>, msg: impl Into<String>) -> CmdError {
        CmdError::Invalid {
            path: path.into(),
            msg: msg.into(),
        }
    }

    /// `Unsupported`.
    pub fn unsupported(msg: impl Into<String>) -> CmdError {
        CmdError::Unsupported { msg: msg.into() }
    }

    /// `Internal`.
    pub fn internal(msg: impl Into<String>) -> CmdError {
        CmdError::Internal { msg: msg.into() }
    }

    /// Every machine code with what it means, in a fixed order (the `error_codes` of the schema
    /// catalog, WP-1N.01). [`CmdError::code`] only returns codes listed here.
    pub const CODES: &'static [(&'static str, &'static str)] = &[
        ("not_found", "An ID or command name does not exist"),
        (
            "locked",
            "The target is locked (a locked track, or soft-locked by another transaction)",
        ),
        (
            "conflict",
            "Stale revision, reused request_id, or a state the request cannot be applied to",
        ),
        (
            "invalid",
            "A param failed schema or semantic validation; the error carries a JSON Pointer",
        ),
        (
            "forbidden",
            "The caller's credential lacks the scope the command needs",
        ),
        ("unsupported", "A valid request this version cannot do"),
        ("internal", "A bug or panic in the engine"),
    ];

    /// The machine code: `not_found`, `locked`, `conflict`, `invalid`, `forbidden`,
    /// `unsupported` or `internal`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            CmdError::NotFound { .. } => "not_found",
            CmdError::Locked { .. } => "locked",
            CmdError::Conflict { .. } => "conflict",
            CmdError::Invalid { .. } => "invalid",
            CmdError::Forbidden { .. } => "forbidden",
            CmdError::Unsupported { .. } => "unsupported",
            CmdError::Internal { .. } => "internal",
        }
    }

    /// The human message (the `Display` form).
    #[must_use]
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Fills in `Conflict.current_rev` if it is not set yet; other variants are unchanged.
    #[must_use]
    pub fn with_rev(mut self, rev: Rev) -> CmdError {
        if let CmdError::Conflict { current_rev, .. } = &mut self
            && current_rev.is_none()
        {
            *current_rev = Some(rev);
        }
        self
    }

    /// Prefixes an `Invalid` path, e.g. with `/steps/3/params` to locate an error inside a
    /// batch; other variants are unchanged.
    #[must_use]
    pub fn under(self, prefix: &str) -> CmdError {
        match self {
            CmdError::Invalid { path, msg } => CmdError::Invalid {
                path: format!("{prefix}{path}"),
                msg,
            },
            other => other,
        }
    }
}

/// Converts a serde error path into a JSON Pointer (RFC 6901).
pub(crate) fn json_pointer(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut out = String::new();
    for segment in path {
        match segment {
            Segment::Seq { index } => {
                out.push('/');
                out.push_str(&index.to_string());
            }
            Segment::Map { key } => {
                out.push('/');
                out.push_str(&crate::path::escape(key));
            }
            Segment::Enum { variant } => {
                out.push('/');
                out.push_str(&crate::path::escape(variant));
            }
            Segment::Unknown => {}
        }
    }
    out
}

/// Deserializes params, mapping failures to `Invalid { path, msg }`. `null` (omitted params)
/// reads as `{}`.
pub(crate) fn decode_params<T: serde::de::DeserializeOwned>(
    params: serde_json::Value,
) -> Result<T, CmdError> {
    let params = if params.is_null() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        params
    };
    serde_path_to_error::deserialize(params).map_err(|e| CmdError::Invalid {
        path: json_pointer(e.path()),
        msg: e.inner().to_string(),
    })
}

/// Serializes an output, mapping a failure (a bug: e.g. a non-string map key) to `Internal`.
pub(crate) fn encode_output<T: Serialize>(value: &T) -> Result<serde_json::Value, CmdError> {
    serde_json::to_value(value)
        .map_err(|e| CmdError::internal(format!("output serialization failed: {e}")))
}

impl Serialize for CmdError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        wire::CmdErrorWire::from(self).serialize(s)
    }
}

impl<'de> Deserialize<'de> for CmdError {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<CmdError, D::Error> {
        wire::CmdErrorWire::deserialize(d).map(CmdError::from)
    }
}

impl JsonSchema for CmdError {
    fn schema_name() -> Cow<'static, str> {
        "CmdError".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        wire::CmdErrorWire::json_schema(generator)
    }
}

/// The wire form of [`CmdError`]: the same variants, each with the human `message` added. The
/// conversions below are exhaustive matches, so the two cannot drift apart.
mod wire {
    use serde::{Deserialize, Serialize};

    use super::{Actor, CmdError, ConflictReason, ResourceKind, Rev, Scope};

    /// A command error: tagged by `code`, with a human `message`.
    #[derive(Serialize, Deserialize, schemars::JsonSchema, specta::Type)]
    #[serde(tag = "code", rename_all = "snake_case")]
    #[schemars(rename = "CmdError")]
    #[specta(inline)]
    pub(super) enum CmdErrorWire {
        /// An ID (or command name) does not exist.
        NotFound {
            /// What kind of thing.
            kind: ResourceKind,
            /// The missing ID or name.
            id: String,
            /// Human message.
            message: String,
        },
        /// The target is locked (a locked track or item, or another connection's transaction).
        Locked {
            /// What kind of thing is locked.
            kind: ResourceKind,
            /// Its ID.
            id: String,
            /// The actor holding a soft lock.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            by: Option<String>,
            /// The transaction holding a soft lock.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            tx: Option<String>,
            /// Human message.
            message: String,
        },
        /// The request conflicts with the current state.
        Conflict {
            /// Machine reason.
            reason: ConflictReason,
            /// The document revision when the error was raised.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            current_rev: Option<Rev>,
            /// When retrying may succeed.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            #[specta(type = Option<specta_typescript::Number>)]
            retry_after_ms: Option<u64>,
            /// The actor holding the soft lock (`item_in_gesture` only).
            #[serde(default, skip_serializing_if = "Option::is_none")]
            holder: Option<Actor>,
            /// Human detail.
            detail: String,
            /// Human message.
            message: String,
        },
        /// A param failed validation; `path` is a JSON Pointer into the params.
        Invalid {
            /// JSON Pointer into the params.
            path: String,
            /// What is wrong.
            msg: String,
            /// Human message.
            message: String,
        },
        /// The caller's credential lacks the scope.
        Forbidden {
            /// The scope the command needs.
            scope: Scope,
            /// What to do instead.
            msg: String,
            /// Human message.
            message: String,
        },
        /// A valid request this version cannot do.
        Unsupported {
            /// What is unsupported.
            msg: String,
            /// Human message.
            message: String,
        },
        /// A bug or a panic in the engine.
        Internal {
            /// What failed.
            msg: String,
            /// Human message.
            message: String,
        },
    }

    impl From<&CmdError> for CmdErrorWire {
        fn from(e: &CmdError) -> CmdErrorWire {
            let message = e.to_string();
            match e.clone() {
                CmdError::NotFound { kind, id } => CmdErrorWire::NotFound { kind, id, message },
                CmdError::Locked { kind, id, by, tx } => CmdErrorWire::Locked {
                    kind,
                    id,
                    by,
                    tx,
                    message,
                },
                CmdError::Conflict {
                    reason,
                    current_rev,
                    retry_after_ms,
                    holder,
                    detail,
                } => CmdErrorWire::Conflict {
                    reason,
                    current_rev,
                    retry_after_ms,
                    holder,
                    detail,
                    message,
                },
                CmdError::Invalid { path, msg } => CmdErrorWire::Invalid { path, msg, message },
                CmdError::Forbidden { scope, msg } => CmdErrorWire::Forbidden {
                    scope,
                    msg,
                    message,
                },
                CmdError::Unsupported { msg } => CmdErrorWire::Unsupported { msg, message },
                CmdError::Internal { msg } => CmdErrorWire::Internal { msg, message },
            }
        }
    }

    impl From<CmdErrorWire> for CmdError {
        fn from(w: CmdErrorWire) -> CmdError {
            match w {
                CmdErrorWire::NotFound { kind, id, .. } => CmdError::NotFound { kind, id },
                CmdErrorWire::Locked {
                    kind, id, by, tx, ..
                } => CmdError::Locked { kind, id, by, tx },
                CmdErrorWire::Conflict {
                    reason,
                    current_rev,
                    retry_after_ms,
                    holder,
                    detail,
                    ..
                } => CmdError::Conflict {
                    reason,
                    current_rev,
                    retry_after_ms,
                    holder,
                    detail,
                },
                CmdErrorWire::Invalid { path, msg, .. } => CmdError::Invalid { path, msg },
                CmdErrorWire::Forbidden { scope, msg, .. } => CmdError::Forbidden { scope, msg },
                CmdErrorWire::Unsupported { msg, .. } => CmdError::Unsupported { msg },
                CmdErrorWire::Internal { msg, .. } => CmdError::Internal { msg },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_form_is_tagged_with_code_and_message() {
        let e = CmdError::invalid("/at", "must be inside the item");
        assert_eq!(e.code(), "invalid");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({
                "code": "invalid",
                "path": "/at",
                "msg": "must be inside the item",
                "message": "invalid value at `/at`: must be inside the item"
            })
        );
        let e = CmdError::not_found(EntityKind::Item, "itm_x");
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"code": "not_found", "kind": "item", "id": "itm_x", "message": "item `itm_x` not found"})
        );
        let e = CmdError::conflict(ConflictReason::StaleRev, "expected rev 3").with_rev(Rev(5));
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({
                "code": "conflict",
                "reason": "stale_rev",
                "current_rev": 5,
                "detail": "expected rev 3",
                "message": "conflict (stale_rev): expected rev 3 (current rev 5)"
            })
        );
    }

    #[test]
    fn every_variant_round_trips_and_has_a_code() {
        let all = [
            CmdError::not_found(ResourceKind::Command, "clip.nope"),
            CmdError::locked(EntityKind::Track, "trk_1"),
            CmdError::Locked {
                kind: ResourceKind::Item,
                id: "itm_1".into(),
                by: Some("agent:claude".into()),
                tx: Some("tx_9".into()),
            },
            CmdError::Conflict {
                reason: ConflictReason::ItemInGesture,
                current_rev: Some(Rev(2)),
                retry_after_ms: Some(250),
                holder: Some(Actor::User),
                detail: "item is being dragged".into(),
            },
            CmdError::invalid("", "params must be an object"),
            CmdError::Forbidden {
                scope: Scope::UiOnly,
                msg: "only the editor UI may set API keys".into(),
            },
            CmdError::unsupported("text tracks"),
            CmdError::internal("boom"),
        ];
        let mut codes = std::collections::BTreeSet::new();
        for e in all {
            let v = serde_json::to_value(&e).unwrap();
            assert_eq!(v["code"], e.code());
            assert_eq!(v["message"], e.message());
            let back: CmdError = serde_json::from_value(v).unwrap();
            assert_eq!(back, e);
            codes.insert(e.code());
        }
        assert_eq!(codes.len(), 7);
    }

    #[test]
    fn messages_are_readable() {
        assert_eq!(
            CmdError::locked(EntityKind::Track, "trk_1").message(),
            "track `trk_1` is locked"
        );
        let soft = CmdError::Locked {
            kind: ResourceKind::Item,
            id: "itm_1".into(),
            by: Some("agent:claude".into()),
            tx: Some("tx_9".into()),
        };
        assert_eq!(
            soft.message(),
            "item `itm_1` is locked by agent:claude (transaction tx_9)"
        );
        assert_eq!(
            CmdError::invalid("", "expected an object").message(),
            "invalid params: expected an object"
        );
        assert_eq!(
            CmdError::invalid("/at", "x").under("/steps/2/params"),
            CmdError::invalid("/steps/2/params/at", "x")
        );
        // `with_rev` keeps an existing revision and ignores other variants.
        let e = CmdError::conflict(ConflictReason::StaleRev, "x").with_rev(Rev(1));
        assert_eq!(e.clone().with_rev(Rev(9)), e);
        assert_eq!(
            CmdError::internal("x").with_rev(Rev(9)),
            CmdError::internal("x")
        );
    }

    #[test]
    fn error_schema_lists_every_code_with_a_message() {
        let schema = schemars::schema_for!(CmdError).to_value();
        let text = schema.to_string();
        for code in [
            "not_found",
            "locked",
            "conflict",
            "invalid",
            "forbidden",
            "unsupported",
            "internal",
        ] {
            assert!(text.contains(&format!("\"{code}\"")), "{code}: {text}");
        }
        assert!(text.contains("\"message\""), "{text}");
    }

    #[test]
    fn rev_is_a_plain_integer() {
        assert_eq!(serde_json::to_value(Rev(7)).unwrap(), json!(7));
        assert_eq!(Rev::ZERO.next(), Rev(1));
        assert_eq!(Rev(u64::MAX).next(), Rev(u64::MAX));
        assert_eq!(Rev(3).to_string(), "3");
    }
}
