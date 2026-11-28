// SPDX-License-Identifier: GPL-3.0-or-later
//! The JSON-RPC surface: a raw WebSocket client of the engine's `/rpc` (what the desktop UI's
//! transport and scripts speak, `docs/reference/rpc.md`), authenticated with the run's `rpc`
//! agent token (actor `agent:rpc`).
//!
//! A call is one request (with a fresh `_meta.request_id`; the result's `_meta` is dropped). A
//! batch is what a client does by hand: `edit.begin_tx{kind: "batch"}`, every call with
//! `_meta.tx` (the client resolves the batch's `$ref`s from the results it already has),
//! `edit.commit_tx`, and `edit.cancel_tx` when a call fails. A lone engine command runs as a
//! plain call, as `cutweave exec` and `cutweave_execute` do.

use cutweave_commands::{EntryKind, Registry};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use crate::rpc_support::{Rpc as Ws, with_meta, with_rid};
use crate::script::{Call, Outcome, StepError, resolve_batch_refs};

/// A JSON-RPC connection.
pub struct Rpc<'a> {
    rt: Runtime,
    ws: Option<Ws>,
    registry: &'a Registry,
    /// The revision the last successful request reported (`result._meta.rev`).
    rev: Option<u64>,
}

impl<'a> Rpc<'a> {
    /// Connects to `127.0.0.1:port/rpc` with `token`.
    pub fn connect(port: u16, token: &str, registry: &'a Registry) -> Rpc<'a> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let ws = rt.block_on(Ws::connect(port, token));
        Rpc {
            rt,
            ws: Some(ws),
            registry,
            rev: None,
        }
    }

    /// One request; the result without `_meta`, or the error's `data` (the `CmdError`).
    fn request(&mut self, method: &str, params: Value) -> Result<Value, StepError> {
        let ws = self.ws.as_mut().expect("an open connection");
        let response = self.rt.block_on(ws.request(method, params));
        if let Some(error) = response.get("error") {
            let raw = error["data"].clone();
            return Err(StepError {
                code: raw["code"].as_str().unwrap_or_default().to_owned(),
                failed_step: None,
                raw: error.clone(),
            });
        }
        let mut result = response["result"].clone();
        self.rev = result["_meta"]["rev"].as_u64();
        if let Value::Object(map) = &mut result {
            map.remove("_meta");
        }
        Ok(result)
    }

    /// The revision the last successful request reported.
    pub fn rev(&self) -> Option<u64> {
        self.rev
    }

    /// `params` with a fresh `_meta.request_id` and, when given, `_meta.expected_rev`.
    fn envelope(params: Value, expected_rev: Option<u64>) -> Value {
        match expected_rev {
            Some(rev) => with_meta(with_rid(params), json!({ "expected_rev": rev })),
            None => with_rid(params),
        }
    }

    /// One call (`_meta.expected_rev` when the step expects a revision; a read takes none).
    pub fn call(&mut self, call: &Call, expected_rev: Option<u64>) -> Outcome {
        let read_only = self
            .registry
            .get(&call.cmd)
            .is_some_and(|e| e.meta.annotations.read_only);
        let expected_rev = expected_rev.filter(|_| !read_only);
        self.request(&call.cmd, Self::envelope(call.args.clone(), expected_rev))
            .map(|out| vec![out])
    }

    fn is_engine(&self, cmd: &str) -> bool {
        self.registry
            .get(cmd)
            .is_some_and(|e| e.kind() == EntryKind::Engine)
    }

    /// A batch as one transaction. The expected revision goes with `edit.begin_tx` and with
    /// `edit.commit_tx`, as `cutweave exec` and `cutweave_execute` send it.
    pub fn batch(&mut self, calls: &[Call], expected_rev: Option<u64>) -> Outcome {
        if let [call] = calls
            && self.is_engine(&call.cmd)
        {
            return self.call(call, expected_rev).map_err(|mut failed| {
                failed.failed_step = Some(0);
                failed
            });
        }
        let opened = self.request(
            "edit.begin_tx",
            Self::envelope(
                json!({ "kind": "batch", "label": "parity rpc batch" }),
                expected_rev,
            ),
        )?;
        let tx = opened["tx"].clone();
        let mut outputs = Vec::with_capacity(calls.len());
        for (i, call) in calls.iter().enumerate() {
            // A reference the client cannot resolve fails the batch before the call, as
            // `invalid` (what `cutweave exec` and `cutweave_execute` report).
            let args = match resolve_batch_refs(&call.args, &outputs) {
                Ok(args) => args,
                Err(msg) => {
                    let _ = self.request("edit.cancel_tx", with_rid(json!({ "tx": tx })));
                    return Err(StepError {
                        code: "invalid".into(),
                        failed_step: u64::try_from(i).ok(),
                        raw: json!({ "client": msg }),
                    });
                }
            };
            let params = with_meta(args, json!({ "tx": tx }));
            match self.request(&call.cmd, with_rid(params)) {
                Ok(out) => outputs.push(out),
                Err(mut failed) => {
                    let _ = self.request("edit.cancel_tx", with_rid(json!({ "tx": tx })));
                    failed.failed_step = u64::try_from(i).ok();
                    return Err(failed);
                }
            }
        }
        if let Err(failed) = self.request(
            "edit.commit_tx",
            Self::envelope(json!({ "tx": tx }), expected_rev),
        ) {
            let _ = self.request("edit.cancel_tx", with_rid(json!({ "tx": tx })));
            return Err(failed);
        }
        Ok(outputs)
    }

    /// Closes the socket.
    pub fn close(mut self) {
        if let Some(ws) = self.ws.take() {
            self.rt.block_on(ws.close());
        }
    }
}
