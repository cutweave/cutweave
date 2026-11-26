// SPDX-License-Identifier: GPL-3.0-or-later
//! JSON-RPC 2.0 framing (WP-1I.02): parsing requests and their `_meta` envelope, building
//! responses and notifications, and mapping [`CmdError`] to JSON-RPC errors.
//!
//! Every error response carries the [`CmdError`] wire form as `error.data`
//! (`{code, message, …}` with `path` for `invalid` and `current_rev` for `conflict`), also for
//! framing errors (parse error, invalid request), so a client always finds a Cutweave machine
//! code in `error.data.code`. `docs/reference/rpc.md` has the table.

use std::str::FromStr;

use cutweave_commands::{CmdError, RequestId, ResourceKind, Rev, TxId};
use serde_json::{Map, Value};

/// The `error.code` values of JSON-RPC error responses.
pub mod codes {
    /// The message is not JSON.
    pub const PARSE_ERROR: i64 = -32_700;
    /// The JSON is not a JSON-RPC 2.0 request object.
    pub const INVALID_REQUEST: i64 = -32_600;
    /// No such command, query or reserved method (`not_found` of kind `command`).
    pub const METHOD_NOT_FOUND: i64 = -32_601;
    /// `invalid`: a param (or `_meta` field) failed validation; `data.path` points at it.
    pub const INVALID_PARAMS: i64 = -32_602;
    /// `internal`: a bug or a panic in the engine.
    pub const INTERNAL_ERROR: i64 = -32_603;
    /// `not_found`: an ID does not exist.
    pub const NOT_FOUND: i64 = -32_001;
    /// `locked`: a locked track or item, or another connection's transaction.
    pub const LOCKED: i64 = -32_002;
    /// `conflict`: stale `expected_rev` (`data.current_rev`), reused `request_id`, undo target
    /// modified, item in another actor's gesture.
    pub const CONFLICT: i64 = -32_003;
    /// `forbidden`: the credential lacks the scope (UI-only commands).
    pub const FORBIDDEN: i64 = -32_004;
    /// `unsupported`: a valid request this engine cannot do.
    pub const UNSUPPORTED: i64 = -32_005;
}

/// The JSON-RPC `error.code` for `e`.
#[must_use]
pub fn error_code(e: &CmdError) -> i64 {
    match e {
        CmdError::NotFound {
            kind: ResourceKind::Command,
            ..
        } => codes::METHOD_NOT_FOUND,
        CmdError::NotFound { .. } => codes::NOT_FOUND,
        CmdError::Locked { .. } => codes::LOCKED,
        CmdError::Conflict { .. } => codes::CONFLICT,
        CmdError::Invalid { .. } => codes::INVALID_PARAMS,
        CmdError::Forbidden { .. } => codes::FORBIDDEN,
        CmdError::Unsupported { .. } => codes::UNSUPPORTED,
        // `Internal`, and any variant a later version adds.
        _ => codes::INTERNAL_ERROR,
    }
}

/// A failed request: the JSON-RPC code and the error it carries as `data`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Failure {
    pub code: i64,
    pub error: CmdError,
}

impl Failure {
    /// The JSON-RPC code [`error_code`] gives `error`.
    pub fn from_cmd(error: CmdError) -> Failure {
        Failure {
            code: error_code(&error),
            error,
        }
    }

    /// A framing error (`PARSE_ERROR`, `INVALID_REQUEST`) carried as `invalid` at `path`.
    pub fn framing(code: i64, path: &str, msg: impl Into<String>) -> Failure {
        Failure {
            code,
            error: CmdError::invalid(path, msg),
        }
    }

    /// The JSON-RPC error object `{code, message, data}`.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("code".into(), Value::from(self.code));
        out.insert("message".into(), Value::String(self.error.message()));
        out.insert(
            "data".into(),
            serde_json::to_value(&self.error).unwrap_or(Value::Null),
        );
        Value::Object(out)
    }
}

impl From<CmdError> for Failure {
    fn from(error: CmdError) -> Failure {
        Failure::from_cmd(error)
    }
}

/// The envelope extension `params._meta` (arch §5.1). There is no actor field: the actor is
/// the connection's credential's (ADR-0016); an `actor` key, like any other unknown key, is
/// ignored.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Meta {
    /// Idempotency key (a UUID; required on mutating calls).
    pub request_id: Option<RequestId>,
    /// Fail with `conflict` (`stale_rev`) unless the document is at this revision.
    pub expected_rev: Option<Rev>,
    /// Run inside this open transaction of the connection.
    pub tx: Option<TxId>,
    /// A gesture update that replaces the earlier ones (spec INV-301, WP-2A.12).
    pub replace: bool,
}

/// A parsed request.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Request {
    /// `None` for a notification (no `id` member): it gets no response.
    pub id: Option<Value>,
    /// The method: a registered name or a reserved `rpc.*` / `project.snapshot` method.
    pub method: String,
    /// The params without `_meta` (`{}` when absent).
    pub params: Map<String, Value>,
    /// The envelope.
    pub meta: Meta,
}

/// A request that could not be parsed, and the response it gets (none for a notification
/// whose framing was valid).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rejected {
    /// The response's `id`; `None` when no response is due (a notification).
    pub id: Option<Value>,
    /// Why.
    pub failure: Failure,
}

fn invalid_request(id: Value, msg: &str) -> Box<Rejected> {
    Box::new(Rejected {
        id: Some(id),
        failure: Failure::framing(codes::INVALID_REQUEST, "", msg),
    })
}

/// Parses one request object (an element of a batch, or a whole message).
pub(crate) fn parse_request(value: Value) -> Result<Request, Box<Rejected>> {
    let Value::Object(mut obj) = value else {
        return Err(invalid_request(
            Value::Null,
            "a request must be a JSON object",
        ));
    };
    // The id first, so later errors can answer it.
    let id = match obj.remove("id") {
        None => None,
        Some(id @ (Value::Null | Value::String(_) | Value::Number(_))) => Some(id),
        Some(_) => {
            return Err(invalid_request(
                Value::Null,
                "`id` must be a string, a number or null",
            ));
        }
    };
    let answer = id.clone().unwrap_or(Value::Null);
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(invalid_request(answer, "`jsonrpc` must be \"2.0\""));
    }
    let method = match obj.remove("method") {
        Some(Value::String(m)) if !m.is_empty() => m,
        _ => {
            return Err(invalid_request(
                answer,
                "`method` must be a non-empty string",
            ));
        }
    };
    let reject = |failure: Failure| {
        Box::new(Rejected {
            id: id.clone(),
            failure,
        })
    };
    let mut params = match obj.remove("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(p)) => p,
        Some(_) => {
            return Err(reject(Failure::from_cmd(CmdError::invalid(
                "",
                "params must be an object (named params)",
            ))));
        }
    };
    let meta = match params.remove("_meta") {
        None | Some(Value::Null) => Meta::default(),
        Some(meta) => parse_meta(meta).map_err(|e| reject(Failure::from_cmd(e)))?,
    };
    Ok(Request {
        id,
        method,
        params,
        meta,
    })
}

fn parse_meta(meta: Value) -> Result<Meta, CmdError> {
    let Value::Object(meta) = meta else {
        return Err(CmdError::invalid("/_meta", "`_meta` must be an object"));
    };
    let mut out = Meta::default();
    match meta.get("request_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => {
            out.request_id = Some(
                RequestId::from_str(s)
                    .map_err(|e| CmdError::invalid("/_meta/request_id", e.to_string()))?,
            );
        }
        Some(_) => {
            return Err(CmdError::invalid(
                "/_meta/request_id",
                "must be a UUID string (a client-generated UUIDv7)",
            ));
        }
    }
    match meta.get("expected_rev") {
        None | Some(Value::Null) => {}
        Some(v) => {
            out.expected_rev = Some(Rev(v.as_u64().ok_or_else(|| {
                CmdError::invalid("/_meta/expected_rev", "must be a non-negative integer")
            })?));
        }
    }
    match meta.get("tx") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => {
            out.tx =
                Some(TxId::from_str(s).map_err(|e| CmdError::invalid("/_meta/tx", e.to_string()))?);
        }
        Some(_) => {
            return Err(CmdError::invalid(
                "/_meta/tx",
                "must be a transaction id string (`tx_<n>`)",
            ));
        }
    }
    match meta.get("replace") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(replace)) => out.replace = *replace,
        Some(_) => {
            return Err(CmdError::invalid(
                "/_meta/replace",
                "must be a boolean (true: this gesture update replaces the earlier ones)",
            ));
        }
    }
    Ok(out)
}

/// `{"jsonrpc":"2.0","id":…,"result":…}`.
pub(crate) fn success(id: Value, result: Value) -> Value {
    let mut out = Map::new();
    out.insert("jsonrpc".into(), Value::from("2.0"));
    out.insert("id".into(), id);
    out.insert("result".into(), result);
    Value::Object(out)
}

/// `{"jsonrpc":"2.0","id":…,"error":{code,message,data}}`.
pub(crate) fn failure(id: Value, failure: &Failure) -> Value {
    let mut out = Map::new();
    out.insert("jsonrpc".into(), Value::from("2.0"));
    out.insert("id".into(), id);
    out.insert("error".into(), failure.to_json());
    Value::Object(out)
}

/// The response to a request with `id` (`None`: a notification, no response).
pub(crate) fn response(id: Option<Value>, outcome: Result<Value, Failure>) -> Option<Value> {
    let id = id?;
    Some(match outcome {
        Ok(result) => success(id, result),
        Err(f) => failure(id, &f),
    })
}

/// The notification text `{"jsonrpc":"2.0","method":…,"params":…}` of an event.
pub(crate) fn notification_text(method: &str, params: Value) -> String {
    let mut out = Map::new();
    out.insert("jsonrpc".into(), Value::from("2.0"));
    out.insert("method".into(), Value::from(method));
    out.insert("params".into(), params);
    Value::Object(out).to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use cutweave_commands::{ConflictReason, Scope};
    use serde_json::json;

    use super::*;

    const RID: &str = "0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f";

    #[test]
    fn parses_a_request_with_its_envelope() {
        let req = parse_request(json!({
            "jsonrpc": "2.0", "id": 7, "method": "clip.split",
            "params": {"item": "itm_1", "at": 5, "_meta": {
                "request_id": RID, "expected_rev": 4, "tx": "tx_9", "replace": true, "actor": "user"
            }}
        }))
        .unwrap();
        assert_eq!(req.id, Some(json!(7)));
        assert_eq!(req.method, "clip.split");
        assert_eq!(Value::Object(req.params), json!({"item": "itm_1", "at": 5}));
        assert_eq!(req.meta.request_id.unwrap().to_string(), RID);
        assert_eq!(req.meta.expected_rev, Some(Rev(4)));
        assert_eq!(req.meta.tx, Some(TxId::new(9)));
        assert!(req.meta.replace);
    }

    #[test]
    fn a_missing_id_is_a_notification_and_null_is_an_id() {
        let note = parse_request(json!({"jsonrpc": "2.0", "method": "rpc.hello"})).unwrap();
        assert_eq!(note.id, None);
        assert!(note.params.is_empty());
        let null = parse_request(json!({"jsonrpc": "2.0", "id": null, "method": "x.y"})).unwrap();
        assert_eq!(null.id, Some(Value::Null));
    }

    #[test]
    fn framing_errors_are_invalid_requests() {
        for bad in [
            json!([1]),
            json!("x"),
            json!({"jsonrpc": "1.0", "id": 1, "method": "a.b"}),
            json!({"id": 1, "method": "a.b"}),
            json!({"jsonrpc": "2.0", "id": 1}),
            json!({"jsonrpc": "2.0", "id": 1, "method": ""}),
            json!({"jsonrpc": "2.0", "id": {"x": 1}, "method": "a.b"}),
        ] {
            let r = parse_request(bad.clone()).unwrap_err();
            assert_eq!(r.failure.code, codes::INVALID_REQUEST, "{bad}");
            assert!(r.id.is_some(), "{bad}: framing errors are always answered");
        }
        // The id is echoed when it could be read.
        let r = parse_request(json!({"jsonrpc": "2.0", "id": "a", "method": 3})).unwrap_err();
        assert_eq!(r.id, Some(json!("a")));
    }

    #[test]
    fn params_and_meta_errors_point_at_the_field() {
        let r =
            parse_request(json!({"jsonrpc": "2.0", "id": 1, "method": "a.b", "params": [1, 2]}))
                .unwrap_err();
        assert_eq!(r.failure.code, codes::INVALID_PARAMS);
        for (meta, path) in [
            (json!(5), "/_meta"),
            (json!({"request_id": "nope"}), "/_meta/request_id"),
            (json!({"request_id": 5}), "/_meta/request_id"),
            (json!({"expected_rev": -1}), "/_meta/expected_rev"),
            (json!({"expected_rev": "3"}), "/_meta/expected_rev"),
            (json!({"tx": "9"}), "/_meta/tx"),
            (json!({"replace": "yes"}), "/_meta/replace"),
        ] {
            let r = parse_request(
                json!({"jsonrpc": "2.0", "id": 1, "method": "a.b", "params": {"_meta": meta}}),
            )
            .unwrap_err();
            assert_eq!(r.failure.code, codes::INVALID_PARAMS);
            assert_eq!(
                r.failure.error,
                CmdError::invalid(
                    path,
                    match &r.failure.error {
                        CmdError::Invalid { msg, .. } => msg.clone(),
                        other => panic!("{other:?}"),
                    }
                )
            );
        }
        // A notification with bad params gets no response.
        let r = parse_request(json!({"jsonrpc": "2.0", "method": "a.b", "params": 3})).unwrap_err();
        assert_eq!(r.id, None);
    }

    #[test]
    fn every_cmd_error_maps_to_a_code_and_carries_its_wire_form() {
        let cases = [
            (
                CmdError::not_found(ResourceKind::Command, "x.y"),
                codes::METHOD_NOT_FOUND,
            ),
            (
                CmdError::not_found(ResourceKind::Item, "itm_1"),
                codes::NOT_FOUND,
            ),
            (
                CmdError::locked(ResourceKind::Track, "trk_1"),
                codes::LOCKED,
            ),
            (
                CmdError::conflict(ConflictReason::StaleRev, "x").with_rev(Rev(9)),
                codes::CONFLICT,
            ),
            (CmdError::invalid("/at", "x"), codes::INVALID_PARAMS),
            (
                CmdError::Forbidden {
                    scope: Scope::UiOnly,
                    msg: "x".into(),
                },
                codes::FORBIDDEN,
            ),
            (CmdError::unsupported("x"), codes::UNSUPPORTED),
            (CmdError::internal("x"), codes::INTERNAL_ERROR),
        ];
        for (e, code) in cases {
            let obj = Failure::from_cmd(e.clone()).to_json();
            assert_eq!(obj["code"], code, "{e:?}");
            assert_eq!(obj["message"], e.message());
            assert_eq!(obj["data"]["code"], e.code());
            assert_eq!(obj["data"], serde_json::to_value(&e).unwrap());
        }
        let conflict =
            Failure::from_cmd(CmdError::conflict(ConflictReason::StaleRev, "x").with_rev(Rev(9)))
                .to_json();
        assert_eq!(conflict["data"]["current_rev"], 9);
        let invalid = Failure::from_cmd(CmdError::invalid("/at", "x")).to_json();
        assert_eq!(invalid["data"]["path"], "/at");
    }

    #[test]
    fn responses_and_notifications_have_the_jsonrpc_shape() {
        assert_eq!(
            response(Some(json!(1)), Ok(json!({"a": 1}))).unwrap(),
            json!({"jsonrpc": "2.0", "id": 1, "result": {"a": 1}})
        );
        let err = response(
            Some(json!("x")),
            Err(Failure::from_cmd(CmdError::unsupported("no"))),
        )
        .unwrap();
        assert_eq!(err["error"]["code"], codes::UNSUPPORTED);
        assert_eq!(response(None, Ok(json!({}))), None);
        let text = notification_text("doc.patched", json!({"rev": 2}));
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            json!({"jsonrpc": "2.0", "method": "doc.patched", "params": {"rev": 2}})
        );
    }
}
