// SPDX-License-Identifier: GPL-3.0-or-later
//! Shared fixtures of the WP-1I.03 media HTTP tests: a full engine serving `/media` (with its
//! counters), a reqwest client, a raw socket for disconnects, and image checks.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cutweave_api::Credential;
use cutweave_api::media::{MediaOptions, MediaStats, MediaStatsSnapshot};
use cutweave_engine::{Connection, Engine, EngineConfig, media_mounts};
use serde_json::{Value, json};

/// A running engine serving `/media`, and how to reach it.
pub struct Server {
    pub engine: Engine,
    pub port: u16,
    /// The `cli` agent token.
    pub token: String,
    pub stats: Arc<MediaStats>,
    pub http: reqwest::Client,
    pub rt: tokio::runtime::Runtime,
}

/// A reply, read to the end.
pub struct Reply {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

impl Server {
    /// Starts `config` with `/media` mounted (`concurrency` slots, counters kept here).
    pub fn start(mut config: EngineConfig, concurrency: usize) -> Server {
        let stats = Arc::new(MediaStats::default());
        config.api.mounts = Some(media_mounts(MediaOptions {
            concurrency,
            stats: Arc::clone(&stats),
        }));
        let engine = Engine::start(config).unwrap();
        let port = engine.port().unwrap();
        let token = engine
            .credentials()
            .unwrap()
            .agent_tokens()
            .iter()
            .find(|a| a.label == "cli")
            .unwrap()
            .token
            .expose()
            .to_owned();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        Server {
            engine,
            port,
            token,
            stats,
            http: reqwest::Client::new(),
            rt,
        }
    }

    pub fn agent(&self) -> Connection {
        self.engine
            .handle()
            .connect(&Credential::Agent("cli".into()))
            .unwrap()
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn stats(&self) -> MediaStatsSnapshot {
        self.stats.snapshot()
    }

    /// `GET path` with the agent token and `headers`.
    pub fn get(&self, path: &str, headers: &[(&str, &str)]) -> Reply {
        let mut all = vec![("authorization", format!("Bearer {}", self.token))];
        all.extend(headers.iter().map(|(k, v)| (*k, (*v).to_owned())));
        self.request("GET", path, &all, None)
    }

    /// `GET path` without any credential (signed URLs).
    pub fn get_anonymous(&self, path: &str) -> Reply {
        self.request("GET", path, &[], None)
    }

    /// `POST /media/sign {paths, ttl_s?}` with the agent token: the signed paths.
    pub fn sign(&self, paths: &[String], ttl_s: Option<u64>) -> Vec<String> {
        let mut body = json!({ "paths": paths });
        if let Some(ttl) = ttl_s {
            body["ttl_s"] = json!(ttl);
        }
        let reply = self.request(
            "POST",
            "/media/sign",
            &[
                ("authorization", format!("Bearer {}", self.token)),
                ("content-type", "application/json".into()),
            ],
            Some(body.to_string()),
        );
        assert_eq!(
            reply.status,
            200,
            "{}",
            String::from_utf8_lossy(&reply.body)
        );
        let out = reply.json();
        out["urls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u.as_str().unwrap().to_owned())
            .collect()
    }

    pub fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<String>,
    ) -> Reply {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut req = self.http.request(method, self.url(path));
        for (k, v) in headers {
            req = req.header(*k, v);
        }
        if let Some(body) = body {
            req = req.body(body);
        }
        self.rt.block_on(async {
            let response = req.send().await.unwrap();
            let status = response.status().as_u16();
            let headers = response.headers().clone();
            let body = response.bytes().await.unwrap().to_vec();
            Reply {
                status,
                headers,
                body,
            }
        })
    }

    /// Sends `GET path` with the agent token on a raw socket, waits until the request is
    /// producing, then closes the socket: how long until nothing is in flight any more.
    pub fn disconnect_mid_request(&self, path: &str, settle: Duration) -> Duration {
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(
            socket,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\r\n",
            self.port, self.token
        )
        .unwrap();
        socket.flush().unwrap();
        wait_until("the request to start producing", || {
            self.stats().in_flight > 0
        });
        std::thread::sleep(settle);
        assert!(self.stats().in_flight > 0, "the request finished too early");
        drop(socket);
        let closed = Instant::now();
        wait_until("the cancelled request to stop", || {
            self.stats().in_flight == 0
        });
        closed.elapsed()
    }

    /// The best of up to `attempts` disconnects (stopping at the first within `budget`), and how
    /// many were made. A cancellation latency is a lower bound that contention only inflates: a
    /// real regression (a decode that does not check its token) is slow on every attempt, a
    /// descheduled thread is slow on one. Each attempt is a fresh request that is cancelled.
    pub fn disconnect_best_of(
        &self,
        path: &str,
        settle: Duration,
        budget: Duration,
        attempts: u32,
    ) -> (Duration, u32) {
        let mut best = Duration::MAX;
        let mut made = 0;
        while made < attempts {
            best = best.min(self.disconnect_mid_request(path, settle));
            made += 1;
            if best < budget {
                break;
            }
        }
        (best, made)
    }
}

/// Polls `f` every millisecond for up to 30 s.
pub fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The size of a JPEG, decoding all of it.
pub fn jpeg_size(bytes: &[u8]) -> (u32, u32) {
    use zune_jpeg::JpegDecoder;
    use zune_jpeg::zune_core::bytestream::ZCursor;
    let mut d = JpegDecoder::new(ZCursor::new(bytes));
    let pixels = d.decode().expect("a decodable JPEG");
    let info = d.info().expect("JPEG info");
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    assert!(pixels.len() >= (w * h) as usize, "every pixel decoded");
    (w, h)
}

/// Whether `bytes` start with the PNG signature.
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

/// The standard base64 `data` of a `render.frame` answer, decoded.
pub fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        }
    };
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        let mut n = 0u32;
        for &c in &chunk[..4 - pad] {
            n = (n << 6) | value(c);
        }
        n <<= 6 * pad;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    out
}
