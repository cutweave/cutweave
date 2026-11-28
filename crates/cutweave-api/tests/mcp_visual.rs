// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.02 acceptance tests: the contact sheet, the filmstrip and the waveform over MCP, on
//! the full engine (fake media, software GPU, null audio) with rmcp's own client, over **both
//! transports** (Streamable HTTP and stdio) and **both spec revisions**:
//!
//! - each picture is one image content block (`image/jpeg`, `image/jpeg`, `image/png`) with the
//!   metadata as `structuredContent` and no base64 in it; a 12-frame contact sheet at default
//!   settings is at most 400 KB;
//! - the names of the items in a contact sheet's cells are labelled `untrusted`;
//! - a large filmstrip is a resource link to the `/media/...` route over HTTP (the route
//!   serves a JPEG to the same token) and stays inline over stdio;
//! - none of the three is a listed tool; `cutweave_search_commands` finds them in a user's
//!   words and `cutweave_describe_command` describes results without the image field.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod mcp_support;

use std::path::PathBuf;
use std::sync::Arc;

use cutweave_api::Credential;
use cutweave_api::mcp::stdio;
use cutweave_api::mcp::{Catalog, CutweaveMcp, Dispatch, McpOptions, Profile};
use cutweave_api::rpc::RpcConfig;
use cutweave_engine::{
    AudioConfig, Call, Connection, Engine, EngineConfig, EngineMode, FullConfig, FullSubsystems,
    GpuConfig, LogConfig,
};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate};
use mcp_support::{Client, Revision, SEC, client_over, http_client};
use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// A full engine with `/mcp`, `/rpc` and `/media` mounted (what the engine process mounts) and
/// one project: three fake 10 s clips, 4 s of each back to back on the video track.
struct Full {
    engine: Engine,
    _dir: tempfile::TempDir,
    token: String,
    port: u16,
    media: Vec<Value>,
}

fn must(conn: &Connection, name: &str, params: Value) -> Value {
    conn.call(Call::new(name, params))
        .unwrap_or_else(|e| panic!("{name} failed: {e}"))
        .output
}

impl Full {
    fn start() -> Full {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("media");
        std::fs::create_dir_all(&folder).unwrap();
        let mut fake = FakeBackend::new();
        let mut clips: Vec<PathBuf> = Vec::new();
        for n in 1..=3 {
            let path = folder.join(format!("clip{n}.mp4"));
            std::fs::write(&path, format!("fake media {n}")).unwrap();
            let d = Duration::from_secs(10).unwrap();
            fake = fake.with_media(
                &path,
                FakeMedia {
                    video: Some(FakeVideo::lasting(320, 180, FrameRate::FPS_30, d)),
                    audio: Some(FakeAudio::tone(SampleRate::HZ_48_000, 2, d)),
                },
            );
            clips.push(path);
        }
        let full = FullSubsystems::new(FullConfig {
            cache_dir: Some(dir.path().join("cache")),
            headless_preview: (160, 90),
            ..FullConfig::default().with_media(Arc::new(fake))
        });
        let mut config = EngineConfig::new(EngineMode::Headless);
        config.port = Some(0);
        config.api.run_dir = Some(dir.path().join("run"));
        config.log = LogConfig::disabled();
        config.allowed_roots = vec![dir.path().to_path_buf()];
        config.gpu = GpuConfig::Software;
        config.audio = AudioConfig::Null;
        config.subsystems = Some(Box::new(full));
        config.api.mounts = Some(cutweave_engine::api_mounts(
            RpcConfig::default(),
            McpOptions::default(),
        ));
        let engine = Engine::start(config).unwrap();
        let token = engine
            .credentials()
            .unwrap()
            .agent_tokens()
            .iter()
            .find(|a| a.label == "mcp")
            .unwrap()
            .token
            .expose()
            .to_owned();
        let port = engine.port().unwrap();
        let conn = engine
            .handle()
            .connect(&Credential::Agent("setup".into()))
            .unwrap();
        let project = must(&conn, "project.new", json!({ "name": "Pictures" }));
        let imported = must(
            &conn,
            "media.import",
            json!({ "paths": clips.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() }),
        );
        assert_eq!(imported["failed"], json!([]), "{imported}");
        for job in imported["job_ids"].as_array().unwrap() {
            let info = must(
                &conn,
                "job.wait",
                json!({ "job_id": job, "timeout_ms": 60_000 }),
            );
            assert_eq!(info["state"], "succeeded", "{info}");
        }
        let media = imported["media_ids"].as_array().unwrap().clone();
        for (n, id) in media.iter().enumerate() {
            must(
                &conn,
                "timeline.insert",
                json!({
                    "sequence": project["sequence"], "track": project["video_track"],
                    "media": id, "at": format!("{}s", n * 4), "duration": "4s",
                }),
            );
        }
        Full {
            engine,
            _dir: dir,
            token,
            port,
            media,
        }
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn http(&self, revision: Revision) -> Client {
        http_client(&format!("{}/mcp", self.origin()), &self.token, revision).await
    }

    /// The handler `cutweave mcp --headless` builds, over an in-memory pipe.
    async fn stdio(&self, revision: Revision) -> Client {
        let handle = self.engine.handle();
        let catalog = Arc::new(Catalog::new(handle.registry(), Profile::Standard));
        let handler = CutweaveMcp::new(
            Arc::new(handle) as Arc<dyn Dispatch>,
            catalog,
            Some(Credential::Agent("mcp".into())),
        );
        let (client_io, server_io) = tokio::io::duplex(4 << 20);
        let (read, write) = tokio::io::split(server_io);
        tokio::spawn(stdio::serve(
            handler,
            read,
            write,
            std::future::pending::<()>(),
        ));
        client_over(client_io, revision).await
    }

    /// `GET path` on the engine's HTTP server with the agent token: status, headers, body.
    async fn get(&self, url: &str) -> (u16, String, Vec<u8>) {
        let path = url
            .strip_prefix(&self.origin())
            .expect("a link to this server");
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\
             Connection: close\r\n\r\n",
            self.port, self.token
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut reply = Vec::new();
        stream.read_to_end(&mut reply).await.unwrap();
        let split = reply.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&reply[..split]).to_lowercase();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, head, reply[split + 4..].to_vec())
    }
}

async fn query(client: &Client, name: &str, args: Value) -> CallToolResult {
    let result = client
        .call("cutweave_query", json!({ "name": name, "args": args }))
        .await;
    assert_ne!(result.is_error, Some(true), "{name}: {result:?}");
    result
}

/// The one image block of `result`: `(mime, bytes)`.
fn image(result: &CallToolResult) -> (String, Vec<u8>) {
    let images: Vec<_> = result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::Image(i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 1, "{result:?}");
    (images[0].mime_type.clone(), base64_decode(&images[0].data))
}

fn links(result: &CallToolResult) -> Vec<(String, Option<String>)> {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            ContentBlock::ResourceLink(link) => Some((link.uri.clone(), link.mime_type.clone())),
            _ => None,
        })
        .collect()
}

fn base64_decode(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes().filter_map(value) {
        acc = (acc << 6) | u32::from(c);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).unwrap());
        }
    }
    out
}

/// The size of a JPEG, from its start-of-frame marker.
fn jpeg_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(bytes[..2], [0xFF, 0xD8], "a JPEG");
    let mut i = 2;
    loop {
        assert_eq!(bytes[i], 0xFF, "a marker");
        let marker = bytes[i + 1];
        let len = usize::from(bytes[i + 2]) << 8 | usize::from(bytes[i + 3]);
        if matches!(marker, 0xC0..=0xC2) {
            let h = u32::from(bytes[i + 5]) << 8 | u32::from(bytes[i + 6]);
            let w = u32::from(bytes[i + 7]) << 8 | u32::from(bytes[i + 8]);
            return (w, h);
        }
        i += 2 + len;
    }
}

/// The size of a PNG, from its header.
fn png_size(bytes: &[u8]) -> (u32, u32) {
    assert_eq!(bytes[..8], [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let be =
        |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    (be(16), be(20))
}

fn size_of(meta: &Value) -> (u32, u32) {
    (
        u32::try_from(meta["width"].as_u64().unwrap()).unwrap(),
        u32::try_from(meta["height"].as_u64().unwrap()).unwrap(),
    )
}

/// Every transport and revision: `(what, client)`.
async fn clients(h: &Full) -> Vec<(String, Client)> {
    let mut out = Vec::new();
    for revision in Revision::ALL {
        out.push((format!("http {revision:?}"), h.http(revision).await));
        out.push((format!("stdio {revision:?}"), h.stdio(revision).await));
    }
    out
}

/// Acceptance: the three pictures on stdio and HTTP, on both revisions.
#[tokio::test]
async fn the_three_pictures_are_image_blocks_on_both_transports_and_revisions() {
    let h = Full::start();
    for (what, client) in clients(&h).await {
        // The contact sheet: 12 frames at default settings.
        let result = query(&client, "render.contact_sheet", json!({ "count": 12 })).await;
        let (mime, jpeg) = image(&result);
        assert_eq!(mime, "image/jpeg", "{what}");
        assert!(jpeg.len() <= 400_000, "{what}: {} bytes", jpeg.len());
        let meta = result.structured_content.clone().unwrap();
        assert!(meta.get("data").is_none(), "{what}: base64 in the metadata");
        assert_eq!(meta["mime"], "image/jpeg");
        assert_eq!(jpeg_size(&jpeg), size_of(&meta), "{what}");
        assert!(size_of(&meta).0 <= 768 && size_of(&meta).1 <= 768);
        let frames = meta["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 12, "{what}");
        assert_eq!(frames[3]["t"], json!(3 * SEC));
        assert_eq!(
            (meta["columns"].clone(), meta["rows"].clone()),
            (json!(4), json!(3))
        );
        // The item of a cell is named, and the name is labelled as text from outside.
        let item = &frames[0]["items_visible"][0];
        assert!(item["item"].as_str().unwrap().starts_with("itm_"), "{what}");
        assert_eq!(
            item["name"],
            json!({ "untrusted": true, "text": "clip1.mp4" }),
            "{what}"
        );
        assert!(result.meta.as_ref().unwrap()["io.cutweave/rev"].is_u64());
        // The text form carries the same metadata for clients that ignore structuredContent.
        let ContentBlock::Text(text) = &result.content[1] else {
            panic!("the JSON as text");
        };
        assert_eq!(serde_json::from_str::<Value>(&text.text).unwrap(), meta);

        // The filmstrip of a clip.
        let result = query(&client, "media.filmstrip", json!({ "media": h.media[0] })).await;
        let (mime, jpeg) = image(&result);
        assert_eq!(mime, "image/jpeg", "{what}");
        let meta = result.structured_content.clone().unwrap();
        assert!(meta.get("data").is_none());
        assert_eq!(jpeg_size(&jpeg), size_of(&meta), "{what}");
        assert_eq!(size_of(&meta), (768, 54));
        assert_eq!(meta["tiles"].as_array().unwrap().len(), 8);
        assert!(meta["link"].as_str().unwrap().starts_with("/media/"));
        assert!(
            links(&result).is_empty(),
            "{what}: a default-size strip is inline"
        );

        // The waveform of its sound.
        let target = json!({ "kind": "media", "media": h.media[0] });
        let result = query(&client, "audio.waveform", json!({ "target": target })).await;
        let (mime, png) = image(&result);
        assert_eq!(mime, "image/png", "{what}");
        let meta = result.structured_content.clone().unwrap();
        assert!(meta.get("data").is_none());
        assert_eq!(png_size(&png), (768, 192), "{what}");
        assert_eq!(size_of(&meta), (768, 192));
        let levels = &meta["audio_levels"];
        assert!(
            levels["peak_db"].as_f64().unwrap() < 0.0,
            "{what}: {levels}"
        );
        assert_eq!(levels["silences"], json!([]));
        assert_eq!(meta["lanes"][0]["channel"], "mix");

        // The mix of the sequence, with its loudness.
        let result = query(
            &client,
            "audio.waveform",
            json!({ "target": { "kind": "range" }, "to": "4s", "loudness": true }),
        )
        .await;
        assert_eq!(image(&result).0, "image/png", "{what}");
        let levels = &result.structured_content.as_ref().unwrap()["audio_levels"];
        assert!(levels["lufs_integrated"].is_number(), "{what}: {levels}");
        assert_eq!(levels["lufs_short_term"].as_array().unwrap().len(), 40);

        // By tool name too, and errors point into `args`.
        let by_tool = client
            .call(
                "cutweave_query",
                json!({ "name": "media_filmstrip", "args": { "media": h.media[1], "count": 2 } }),
            )
            .await;
        assert_eq!(image(&by_tool).0, "image/jpeg", "{what}");
        let e = client
            .err(
                "cutweave_query",
                json!({ "name": "media.filmstrip", "args": { "media": h.media[0], "count": 0 } }),
            )
            .await;
        assert_eq!(e["error"]["code"], "invalid", "{what}");
        assert_eq!(e["error"]["path"], "/args/count", "{what}");
        client.close().await;
    }
}

/// A filmstrip above 1568 px is a link to the `/media` route over HTTP, which serves the JPEG;
/// over stdio, with no HTTP origin to link to, it stays inline.
#[tokio::test]
async fn a_large_filmstrip_is_a_media_link_over_http_and_inline_over_stdio() {
    let h = Full::start();
    let args = json!({ "media": h.media[0], "max_size": 2048, "count": 16 });
    for revision in Revision::ALL {
        let client = h.http(revision).await;
        let result = query(&client, "media.filmstrip", args.clone()).await;
        assert!(
            !result
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::Image(_))),
            "{revision:?}: no image next to the link"
        );
        let links = links(&result);
        assert_eq!(links.len(), 1, "{revision:?}");
        let (uri, mime) = &links[0];
        assert_eq!(mime.as_deref(), Some("image/jpeg"));
        let meta = result.structured_content.clone().unwrap();
        assert!(meta.get("data").is_none());
        assert_eq!(
            *uri,
            format!("{}{}", h.origin(), meta["link"].as_str().unwrap())
        );
        assert_eq!(size_of(&meta), (2048, 72));
        // The route serves that strip to the same token, under the ETag the result names.
        let (status, head, body) = h.get(uri).await;
        assert_eq!(status, 200, "{uri}: {head}");
        assert!(head.contains("content-type: image/jpeg"), "{head}");
        assert!(
            head.contains(&format!("etag: \"{}\"", meta["etag"].as_str().unwrap())),
            "{head}"
        );
        assert_eq!(jpeg_size(&body), (2048, 72));
        // A part of a clip has no route: inline, whatever its size.
        let part = query(
            &client,
            "media.filmstrip",
            json!({ "media": h.media[0], "max_size": 2048, "count": 16, "to": "5s" }),
        )
        .await;
        assert_eq!(jpeg_size(&image(&part).1), (2048, 72));
        // So has a contact sheet.
        let sheet = query(
            &client,
            "render.contact_sheet",
            json!({ "count": 2, "max_size": 2048 }),
        )
        .await;
        assert_eq!(image(&sheet).0, "image/jpeg");
        client.close().await;

        let client = h.stdio(revision).await;
        let result = query(&client, "media.filmstrip", args.clone()).await;
        assert_eq!(
            jpeg_size(&image(&result).1),
            (2048, 72),
            "stdio {revision:?}"
        );
        client.close().await;
    }
}

/// None of the three is listed; search finds them in a user's words; describe gives the result
/// schema as MCP returns it; none runs inside a `cutweave_execute` batch.
#[tokio::test]
async fn the_visual_commands_are_found_described_and_kept_out_of_the_list() {
    let h = Full::start();
    let client = h.http(Revision::Stateful).await;
    let tools = client.service.list_tools(None).await.unwrap().tools;
    assert_eq!(tools.len(), 32, "the standard list is as it was");
    for tool in ["render_contact_sheet", "media_filmstrip", "audio_waveform"] {
        assert!(tools.iter().all(|t| t.name != tool), "{tool} is listed");
    }
    for (phrase, command) in [
        ("thumbnail grid", "render.contact_sheet"),
        ("see the edit", "render.contact_sheet"),
        ("contact sheet", "render.contact_sheet"),
        ("waveform picture", "audio.waveform"),
        ("where is the silence", "audio.waveform"),
        ("thumbnails of the footage", "media.filmstrip"),
        ("look inside the clip", "media.filmstrip"),
    ] {
        let found = client
            .ok(
                "cutweave_search_commands",
                json!({ "query": phrase, "limit": 3 }),
            )
            .await;
        let names: Vec<&str> = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&command), "\"{phrase}\": {names:?}");
        let hit = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == command)
            .unwrap();
        assert!(
            hit["call"].as_str().unwrap().contains("cutweave_query"),
            "{hit}"
        );
    }
    let described = client
        .ok(
            "cutweave_describe_command",
            json!({ "names": ["render.contact_sheet", "media.filmstrip", "audio.waveform"] }),
        )
        .await;
    let commands = described["commands"].as_array().unwrap();
    assert_eq!(commands.len(), 3);
    for command in commands {
        let output = &command["output_schema"];
        assert!(
            output["properties"].get("data").is_none(),
            "{}",
            command["name"]
        );
        assert!(output["properties"].get("mime").is_some());
        assert_eq!(command["annotations"]["readOnlyHint"], true);
    }
    let sheet = commands
        .iter()
        .find(|c| c["name"] == "render.contact_sheet")
        .unwrap();
    assert!(
        sheet["output_schema"].to_string().contains("\"untrusted\""),
        "the item names are described as untrusted text"
    );

    // Pictures show the committed document: not inside a batch.
    let refused = client
        .execute(json!([
            { "cmd": "edit.checkpoint", "args": {} },
            { "cmd": "media.filmstrip", "args": { "media": h.media[0] } },
        ]))
        .await;
    assert_eq!(refused.is_error, Some(true));
    let refused = client
        .execute(json!([{ "cmd": "render.contact_sheet", "args": {} }]))
        .await;
    assert_eq!(refused.is_error, Some(true));
    client.close().await;
}
