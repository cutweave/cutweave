//! WebSocket control channel (`ws://127.0.0.1:<port>/`): transport commands from the
//! renderer (play / pause / seek / format / resize), authenticated with the one-time UI token.
//!
//! Spike stand-in for the JSON-RPC surface of arch §6. Defences kept from §6: localhost bind,
//! Host + Origin checks (DNS rebinding), token as the first message (never in the URL).
//!
//! Commands are forwarded over an mpsc channel that the render loop blocks on between frames,
//! so a seek or resize wakes it immediately; the loop applies them latest-wins (only the last
//! seek and the last resize of a burst are rendered).

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

/// Commands forwarded to the render loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Start playback from the current position.
    Play,
    /// Pause at the current position.
    Pause,
    /// Jump to `pts` flicks (latest wins). While paused this renders the frame on demand,
    /// low-res first, then the full-resolution refine (arch §10 step 5).
    Seek(i64),
    /// Switch the output pixel format.
    Format(s1_ring::PixelFormat),
    /// Render at this size from now on (the preview canvas size in device pixels).
    Resize(u32, u32),
    /// Scrub tuning: low-res preview divisor (0 or 1 = off) and the emulated full-resolution
    /// decode time per frame.
    ScrubConfig {
        /// Each dimension of the scrub preview is divided by this.
        lowres_div: u32,
        /// Emulated full-resolution decode cost in ms.
        full_cost_ms: u32,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMsg {
    Auth {
        token: String,
    },
    Play,
    Pause,
    Seek {
        pts: i64,
    },
    Format {
        format: String,
    },
    Resize {
        width: u32,
        height: u32,
    },
    #[serde(rename = "scrub-config")]
    ScrubConfig {
        lowres_div: u32,
        full_cost_ms: u32,
    },
    Ping {
        t: f64,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerMsg<'a> {
    Hello { pid: u32 },
    Ack { cmd: &'a str },
    Pong { t: f64, engine_us: u64 },
    Error { message: String },
}

/// Settings shared by all connections.
#[derive(Clone)]
pub struct ControlConfig {
    /// The one-time UI token (hex).
    pub token: String,
    /// Bound port (for the Host check).
    pub port: u16,
    /// Allowed `Origin` header value for browser clients (e.g. `app://s1`).
    pub allow_origin: String,
}

/// Accept connections forever on a background thread.
pub fn spawn(listener: TcpListener, cfg: ControlConfig, tx: Sender<Command>) {
    std::thread::Builder::new()
        .name("ws-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        let (cfg, tx) = (cfg.clone(), tx.clone());
                        let _ =
                            std::thread::Builder::new()
                                .name("ws-conn".into())
                                .spawn(move || {
                                    if let Err(e) = serve(s, &cfg, &tx) {
                                        tracing::info!("control connection closed: {e}");
                                    }
                                });
                    }
                    Err(e) => tracing::warn!("accept failed: {e}"),
                }
            }
        })
        .ok();
}

fn forbidden(msg: &str) -> ErrorResponse {
    let mut r = ErrorResponse::new(Some(msg.to_owned()));
    *r.status_mut() = StatusCode::FORBIDDEN;
    r
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn send(ws: &mut WebSocket<TcpStream>, msg: &ServerMsg<'_>) -> tungstenite::Result<()> {
    let text = serde_json::to_string(msg).unwrap_or_default();
    ws.send(Message::text(text))
}

// The handshake callback's error type (an HTTP response) is fixed by tungstenite.
#[allow(clippy::result_large_err)]
fn serve(stream: TcpStream, cfg: &ControlConfig, tx: &Sender<Command>) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let hosts = [
        format!("127.0.0.1:{}", cfg.port),
        format!("localhost:{}", cfg.port),
    ];
    let mut ws = tungstenite::accept_hdr(stream, |req: &Request, resp: Response| {
        let host = req
            .headers()
            .get("host")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");
        if !hosts.iter().any(|h| h == host) {
            return Err(forbidden("bad host"));
        }
        // Browsers always send Origin; non-browser clients (tests, CLI) may omit it but still
        // need the token.
        if let Some(origin) = req.headers().get("origin")
            && origin.to_str().ok() != Some(cfg.allow_origin.as_str())
        {
            return Err(forbidden("bad origin"));
        }
        Ok(resp)
    })
    .map_err(|e| anyhow::anyhow!("handshake: {e}"))?;

    // First message must be the token, within the 2 s read timeout.
    let authed = match ws.read()? {
        Message::Text(t) => matches!(
            serde_json::from_str::<ClientMsg>(t.as_str()),
            Ok(ClientMsg::Auth { token }) if constant_time_eq(token.as_bytes(), cfg.token.as_bytes())
        ),
        _ => false,
    };
    if !authed {
        let _ = send(
            &mut ws,
            &ServerMsg::Error {
                message: "unauthorized".into(),
            },
        );
        let _ = ws.close(None);
        anyhow::bail!("unauthorized client");
    }
    ws.get_mut().set_read_timeout(None)?;
    send(
        &mut ws,
        &ServerMsg::Hello {
            pid: std::process::id(),
        },
    )?;
    tracing::info!("control client authenticated");

    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::ConnectionReset => {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let Message::Text(text) = msg else { continue };
        let (cmd, name) = match serde_json::from_str::<ClientMsg>(text.as_str()) {
            Ok(ClientMsg::Play) => (Some(Command::Play), "play"),
            Ok(ClientMsg::Pause) => (Some(Command::Pause), "pause"),
            Ok(ClientMsg::Seek { pts }) => (Some(Command::Seek(pts)), "seek"),
            Ok(ClientMsg::Resize { width, height }) => {
                (Some(Command::Resize(width, height)), "resize")
            }
            Ok(ClientMsg::ScrubConfig {
                lowres_div,
                full_cost_ms,
            }) => (
                Some(Command::ScrubConfig {
                    lowres_div,
                    full_cost_ms: full_cost_ms.min(2000),
                }),
                "scrub-config",
            ),
            Ok(ClientMsg::Format { format }) => match format.as_str() {
                "rgba8" => (Some(Command::Format(s1_ring::PixelFormat::Rgba8)), "format"),
                "nv12" => (Some(Command::Format(s1_ring::PixelFormat::Nv12)), "format"),
                _ => {
                    send(
                        &mut ws,
                        &ServerMsg::Error {
                            message: format!("bad format {format}"),
                        },
                    )?;
                    continue;
                }
            },
            Ok(ClientMsg::Ping { t }) => {
                send(
                    &mut ws,
                    &ServerMsg::Pong {
                        t,
                        engine_us: s1_ring::clock::now_us(),
                    },
                )?;
                continue;
            }
            Ok(ClientMsg::Auth { .. }) => (None, "auth"),
            Err(e) => {
                send(
                    &mut ws,
                    &ServerMsg::Error {
                        message: e.to_string(),
                    },
                )?;
                continue;
            }
        };
        if let Some(cmd) = cmd
            && tx.send(cmd).is_err()
        {
            return Ok(());
        }
        send(&mut ws, &ServerMsg::Ack { cmd: name })?;
    }
}
