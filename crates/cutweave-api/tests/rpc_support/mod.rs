// SPDX-License-Identifier: GPL-3.0-or-later
//! A JSON-RPC WebSocket test client (tokio-tungstenite) for the WP-1I.02 tests.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

/// How long a test waits for any one message.
pub const WAIT: Duration = Duration::from_secs(20);

/// A fresh request id (a UUID, unique in this process and across runs).
pub fn request_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    #[allow(clippy::cast_possible_truncation)]
    let tail = (base as u64) ^ (n << 40) ^ n;
    format!(
        "0190f5c2-{:04x}-7{:03x}-8{:03x}-{:012x}",
        std::process::id() & 0xFFFF,
        (n >> 12) & 0xFFF,
        n & 0xFFF,
        tail & 0xFFFF_FFFF_FFFF
    )
}

/// `params` with `_meta.request_id` set to a fresh id.
pub fn with_rid(params: Value) -> Value {
    with_meta(params, json!({ "request_id": request_id() }))
}

/// `params` with `_meta` merged in.
pub fn with_meta(params: Value, meta: Value) -> Value {
    let mut params = match params {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let entry = params
        .entry("_meta")
        .or_insert_with(|| Value::Object(Map::new()));
    if let (Value::Object(into), Value::Object(from)) = (entry, meta) {
        into.extend(from);
    }
    Value::Object(params)
}

/// The upgrade request for `ws://127.0.0.1:<port>/rpc` with a bearer token.
pub fn upgrade_request(port: u16, token: &str) -> Request {
    let mut req = format!("ws://127.0.0.1:{port}/rpc")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    req
}

/// One WebSocket connection speaking JSON-RPC.
pub struct Rpc {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// Notifications received while waiting for a response, oldest first.
    pub notes: VecDeque<Value>,
    /// Whether the server sent a close frame.
    pub closed: bool,
    /// The server's close frame, if it carried one.
    pub close_frame: Option<CloseFrame>,
}

impl Rpc {
    /// Connects with a bearer token.
    pub async fn connect(port: u16, token: &str) -> Rpc {
        Rpc::connect_with(upgrade_request(port, token)).await.0
    }

    /// Connects with any upgrade request.
    pub async fn connect_with(req: Request) -> (Rpc, Response) {
        let (ws, response) = connect_async(req).await.unwrap();
        (
            Rpc {
                ws,
                next_id: 1,
                notes: VecDeque::new(),
                closed: false,
                close_frame: None,
            },
            response,
        )
    }

    /// Sends a raw text frame.
    pub async fn send_text(&mut self, text: &str) {
        self.ws.send(Message::text(text)).await.unwrap();
    }

    /// Sends a raw message.
    pub async fn send_message(&mut self, msg: Message) {
        self.ws.send(msg).await.unwrap();
    }

    /// Sends `value` as one text frame.
    pub async fn send(&mut self, value: &Value) {
        self.send_text(&value.to_string()).await;
    }

    /// The next JSON message (`None` once the server closed the connection).
    pub async fn next(&mut self) -> Option<Value> {
        self.next_within(WAIT).await.expect("no message in time")
    }

    /// The next JSON message within `wait`: `Err` on timeout, `Ok(None)` once closed.
    pub async fn next_within(&mut self, wait: Duration) -> Result<Option<Value>, ()> {
        loop {
            let msg = tokio::time::timeout(wait, self.ws.next())
                .await
                .map_err(|_| ())?;
            match msg {
                None | Some(Err(_)) => return Ok(None),
                Some(Ok(Message::Text(text))) => {
                    return Ok(Some(serde_json::from_str(text.as_str()).unwrap()));
                }
                Some(Ok(Message::Close(frame))) => {
                    self.closed = true;
                    self.close_frame = frame;
                }
                Some(Ok(_)) => {}
            }
        }
    }

    /// Sends a request with a fresh numeric id; returns the id.
    pub async fn start(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        id
    }

    /// Waits for the response to `id`, keeping notifications in [`notes`](Self::notes).
    pub async fn response(&mut self, id: u64) -> Value {
        loop {
            let Some(msg) = self.next().await else {
                panic!("the connection closed ({:?})", self.close_frame);
            };
            if msg.get("method").is_some() && msg.get("id").is_none() {
                self.notes.push_back(msg);
                continue;
            }
            assert_eq!(msg["id"], id, "unexpected response {msg}");
            return msg;
        }
    }

    /// Sends a request and returns the whole response object.
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.start(method, params).await;
        self.response(id).await
    }

    /// The result of a call that must succeed.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let r = self.request(method, params.clone()).await;
        assert!(
            r.get("error").is_none(),
            "{method} {params} failed: {}",
            r["error"]
        );
        r["result"].clone()
    }

    /// The result of a mutating call (with a fresh `_meta.request_id`) that must succeed.
    pub async fn mutate(&mut self, method: &str, params: Value) -> Value {
        self.call(method, with_rid(params)).await
    }

    /// The error object of a call that must fail.
    pub async fn error(&mut self, method: &str, params: Value) -> Value {
        let r = self.request(method, params.clone()).await;
        assert!(
            r.get("result").is_none(),
            "{method} {params} succeeded: {r}"
        );
        r["error"].clone()
    }

    /// The params of the next notification named `name` (stashed ones first; others are
    /// skipped).
    pub async fn event(&mut self, name: &str) -> Value {
        if let Some(i) = self.notes.iter().position(|n| n["method"] == name) {
            let note = self.notes.remove(i).unwrap();
            return note["params"].clone();
        }
        loop {
            let Some(msg) = self.next().await else {
                panic!("the connection closed ({:?})", self.close_frame);
            };
            if msg["method"] == name {
                return msg["params"].clone();
            }
            if msg.get("method").is_some() {
                self.notes.push_back(msg);
            }
        }
    }

    /// Subscribes to `events`.
    pub async fn subscribe(&mut self, events: &[&str]) -> Value {
        self.call("rpc.subscribe", json!({ "events": events }))
            .await
    }

    /// Reads until the server closes the connection; returns its close frame (`None` when the
    /// connection ended without one).
    pub async fn until_closed(&mut self) -> Option<CloseFrame> {
        while !self.closed {
            let msg = tokio::time::timeout(WAIT, self.ws.next())
                .await
                .expect("the server did not close the connection");
            match msg {
                None | Some(Err(_)) => break,
                Some(Ok(Message::Close(frame))) => {
                    self.closed = true;
                    self.close_frame = frame;
                }
                Some(Ok(_)) => {}
            }
        }
        self.close_frame.clone()
    }

    /// Closes the connection from the client side.
    pub async fn close(mut self) {
        let _ = self.ws.close(None).await;
        // Drain until the server's close reply (or the end of the stream).
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(Ok(_)) = self.ws.next().await {}
        })
        .await;
    }
}

/// The numeric close code of `frame`.
pub fn close_code(frame: &CloseFrame) -> u16 {
    u16::from(frame.code)
}
