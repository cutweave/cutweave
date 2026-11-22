//! Typed command errors (arch §5.1): machine `code` + fields, plus a human `message` on the wire.

use std::fmt;

use serde_json::Value;

use crate::{tx::Rev, wire};

/// What kind of thing was not found.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// A registry entry (unknown command name).
    Command,
    /// Media asset.
    Media,
    /// Sequence.
    Sequence,
    /// Track.
    Track,
    /// Timeline item.
    Item,
}

impl fmt::Display for EntityKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            EntityKind::Command => "command",
            EntityKind::Media => "media",
            EntityKind::Sequence => "sequence",
            EntityKind::Track => "track",
            EntityKind::Item => "item",
        })
    }
}

/// Machine-readable conflict reason.
#[wire]
#[derive(Copy, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// `expected_rev` did not match the current revision.
    StaleRev,
    /// A `request_id` was reused with a different payload.
    RequestIdReused,
    /// The edit would overlap another item on the same track.
    Overlap,
    /// Undo/redo target was changed by a later transaction (another actor).
    HistoryTargetModified,
}

impl fmt::Display for ConflictReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConflictReason::StaleRev => "stale_rev",
            ConflictReason::RequestIdReused => "request_id_reused",
            ConflictReason::Overlap => "overlap",
            ConflictReason::HistoryTargetModified => "history_target_modified",
        })
    }
}

/// Every error a command, query or engine op can return.
#[wire]
#[derive(thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum CmdError {
    /// An id (or command name) does not exist.
    #[error("{entity} `{id}` not found")]
    NotFound {
        /// Kind.
        entity: EntityKind,
        /// The missing id.
        id: String,
    },
    /// A param failed validation. `path` is a JSON Pointer into the request params.
    #[error("invalid value at `{path}`: {msg}")]
    Invalid {
        /// JSON Pointer (RFC 6901), `""` for the params object itself.
        path: String,
        /// What is wrong.
        msg: String,
    },
    /// The request conflicts with the current state.
    #[error("conflict ({reason}): {detail}")]
    Conflict {
        /// Machine reason.
        reason: ConflictReason,
        /// Human detail.
        detail: String,
        /// Revision at the time of the error (filled in by the engine).
        current_rev: Option<Rev>,
    },
    /// Valid request the engine cannot do (yet).
    #[error("unsupported: {msg}")]
    Unsupported {
        /// What is unsupported.
        msg: String,
    },
}

impl CmdError {
    /// `Invalid` at a JSON Pointer.
    pub fn invalid(path: impl Into<String>, msg: impl Into<String>) -> CmdError {
        CmdError::Invalid {
            path: path.into(),
            msg: msg.into(),
        }
    }

    /// `Conflict` (the engine fills `current_rev`).
    pub fn conflict(reason: ConflictReason, detail: impl Into<String>) -> CmdError {
        CmdError::Conflict {
            reason,
            detail: detail.into(),
            current_rev: None,
        }
    }

    /// Machine code (`not_found`, `invalid`, `conflict`, `unsupported`).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            CmdError::NotFound { .. } => "not_found",
            CmdError::Invalid { .. } => "invalid",
            CmdError::Conflict { .. } => "conflict",
            CmdError::Unsupported { .. } => "unsupported",
        }
    }

    /// Prefixes an `Invalid` path (used to locate errors inside batches).
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

    /// Wire form: the tagged error plus a human `message`.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let mut v = serde_json::to_value(self)
            .unwrap_or_else(|_| serde_json::json!({ "code": self.code() }));
        if let Value::Object(map) = &mut v {
            map.insert("message".into(), Value::String(self.to_string()));
        }
        v
    }
}

/// Converts a serde path into a JSON Pointer (RFC 6901).
pub(crate) fn json_pointer(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut out = String::new();
    for seg in path {
        match seg {
            Segment::Seq { index } => {
                out.push('/');
                out.push_str(&index.to_string());
            }
            Segment::Map { key } => {
                out.push('/');
                out.push_str(&key.replace('~', "~0").replace('/', "~1"));
            }
            Segment::Enum { variant } => {
                out.push('/');
                out.push_str(variant);
            }
            Segment::Unknown => {}
        }
    }
    out
}

/// Deserializes params, mapping failures to `Invalid { path, msg }`.
pub(crate) fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, CmdError> {
    serde_path_to_error::deserialize(params).map_err(|e| CmdError::Invalid {
        path: json_pointer(e.path()),
        msg: e.inner().to_string(),
    })
}

/// Serializes an output (infallible for our types; mapped defensively).
pub(crate) fn encode<T: serde::Serialize>(value: &T) -> Result<Value, CmdError> {
    serde_json::to_value(value).map_err(|e| CmdError::Unsupported {
        msg: format!("output serialization failed: {e}"),
    })
}
