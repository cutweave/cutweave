// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1J.02 acceptance tests: `render_frame` image content, the resources and their change
//! notifications, on the full engine (fake media, software GPU, null audio) over loopback HTTP
//! with rmcp's own client on both spec revisions.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    missing_docs
)]

mod mcp_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use cutweave_api::Credential;
use cutweave_api::mcp::{ListSize, McpOptions, Profile, STANDARD_BUDGET};
use cutweave_engine::{
    AudioConfig, Call, Connection, Engine, EngineConfig, EngineMode, FullConfig, FullSubsystems,
    GpuConfig, LogConfig,
};
use cutweave_media::testing::{FakeAudio, FakeBackend, FakeMedia, FakeVideo};
use cutweave_model::{Duration, FrameRate, SampleRate};
use mcp_support::{Revision, SEC, http_client};
use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt as _, RoleClient, ServiceExt as _,
    model::{
        CallToolRequestParams, CallToolResult, ClientConfig, ContentBlock, ProtocolVersion,
        ReadResourceRequestParams, ResourceContents, ResourceUpdatedNotificationParam,
        ServerNotification, SubscribeRequestParams, SubscriptionFilter,
    },
    service::{NotificationContext, RunningService},
    transport::{
        StreamableHttpClientTransport, streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// A full engine with the production MCP mount and three fake 10 s clips.
struct Full {
    engine: Engine,
    _dir: tempfile::TempDir,
    token: String,
    port: u16,
    clips: Vec<PathBuf>,
}

impl Full {
    fn start() -> Full {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let mut fake = FakeBackend::new();
        let mut clips = Vec::new();
        for n in 1..=3 {
            let path = media.join(format!("clip{n}.mp4"));
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
        config.api.mounts = Some(cutweave_engine::mcp_mounts(McpOptions::default()));
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
        Full {
            engine,
            _dir: dir,
            token,
            port,
            clips,
        }
    }

    fn url(&self, profile: Profile) -> String {
        let route = match profile.route() {
            "/" => "",
            route => route,
        };
        format!("http://127.0.0.1:{}/mcp{route}", self.port)
    }

    fn direct(&self) -> Connection {
        self.engine
            .handle()
            .connect(&Credential::Agent("setup".into()))
            .unwrap()
    }
}

fn must(conn: &Connection, name: &str, params: Value) -> Value {
    conn.call(Call::new(name, params))
        .unwrap_or_else(|e| panic!("{name} failed: {e}"))
        .output
}

/// Records `notifications/resources/updated` and asks for one protocol revision.
#[derive(Clone)]
struct Watching {
    version: ProtocolVersion,
    updates: mpsc::UnboundedSender<String>,
}

impl ClientHandler for Watching {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.version.clone())
    }

    async fn on_resource_updated(
        &self,
        params: ResourceUpdatedNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let _ = self.updates.send(params.uri);
    }
}

struct Agent {
    service: RunningService<RoleClient, Watching>,
    updates: mpsc::UnboundedReceiver<String>,
}

impl Agent {
    async fn connect(h: &Full, profile: Profile, revision: Revision) -> Agent {
        let (tx, updates) = mpsc::unbounded_channel();
        let handler = Watching {
            version: revision.version(),
            updates: tx,
        };
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(h.url(profile)).auth_header(&h.token),
        );
        let service = match revision {
            Revision::Stateful => handler.serve(transport).await.expect("initialize"),
            Revision::Stateless => handler
                .serve_with_lifecycle(
                    transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![revision.version()],
                    },
                )
                .await
                .expect("server/discover"),
        };
        Agent { service, updates }
    }

    async fn call(&self, tool: &str, args: Value) -> CallToolResult {
        let Value::Object(args) = args else {
            panic!("object args")
        };
        self.service
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(args))
            .await
            .expect("tools/call")
    }

    async fn ok(&self, tool: &str, args: Value) -> Value {
        let r = self.call(tool, args).await;
        assert_ne!(r.is_error, Some(true), "{tool}: {r:?}");
        r.structured_content.unwrap()
    }
}

/// The fixture: a new project with three 4 s clips back to back on the video track.
/// Returns `(sequence, track, the clip ids each insert made)`.
fn fixture(h: &Full) -> (Value, Value, Vec<Value>) {
    let conn = h.direct();
    let project = must(&conn, "project.new", json!({ "name": "Frames" }));
    let (sequence, track) = (project["sequence"].clone(), project["video_track"].clone());
    let imported = must(
        &conn,
        "media.import",
        json!({ "paths": h.clips.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() }),
    );
    assert_eq!(imported["failed"], json!([]), "{imported}");
    let mut clips = Vec::new();
    for (n, media) in imported["media_ids"].as_array().unwrap().iter().enumerate() {
        let inserted = must(
            &conn,
            "timeline.insert",
            json!({
                "sequence": sequence, "track": track, "media": media,
                "at": format!("{}s", n * 4), "duration": "4s",
            }),
        );
        clips.push(inserted["clips"].clone());
    }
    (sequence, track, clips)
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
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        let len = usize::from(bytes[i + 2]) << 8 | usize::from(bytes[i + 3]);
        if matches!(marker, 0xC0..=0xC2) {
            let h = u32::from(bytes[i + 5]) << 8 | u32::from(bytes[i + 6]);
            let w = u32::from(bytes[i + 7]) << 8 | u32::from(bytes[i + 8]);
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

#[tokio::test]
async fn the_standard_list_fits_the_standard_budget_with_render_frame() {
    let h = Full::start();
    for profile in [Profile::Standard, Profile::Minimal] {
        for revision in Revision::ALL {
            let client = http_client(&h.url(profile), &h.token, revision).await;
            let tools = client.service.list_tools(None).await.unwrap().tools;
            let size = ListSize::of(&tools);
            println!(
                "SIZE {profile:?} {revision:?}: {} tools, {} bytes, ~{} tokens",
                size.tools,
                size.bytes,
                size.tokens()
            );
            if profile == Profile::Standard {
                // WP-2I.01: the whole core table (`mcp_core.toml`) and the 4 generic tools.
                assert_eq!(size.tools, 32);
                assert!(STANDARD_BUDGET.fits(size), "{size:?}");
                let table = cutweave_api::mcp::core::CoreTable::get();
                let listed: Vec<&str> = tools.iter().map(|t| &*t.name).collect();
                let expected: Vec<&str> = table
                    .tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .chain(cutweave_api::mcp::catalog::GENERIC_TOOLS)
                    .collect();
                assert_eq!(listed, expected);
                for tool in &tools {
                    let Some(entry) = table.tools.iter().find(|t| t.name == *tool.name) else {
                        continue;
                    };
                    assert_eq!(tool.description.as_deref(), Some(&*entry.listing()));
                }
                // `render_frame`'s example is a valid call (the other examples: tests/mcp.rs).
                let frame = table.by_command("render.frame").unwrap();
                let registry = h.engine.handle().registry();
                let schema = &registry.get("render.frame").unwrap().meta.input_schema;
                assert_eq!(
                    mcp_support::schema_error(schema, &mcp_support::example_args(frame), ""),
                    None
                );
            } else {
                assert!(profile.budget().fits(size), "{size:?}");
                assert!(tools.iter().any(|t| t.name == "render_frame"));
            }
            client.close().await;
        }
    }
}

/// Acceptance 1: `render_frame` on the 3-clip fixture at t = 2 s.
#[tokio::test]
async fn render_frame_returns_one_jpeg_image_block_and_the_metadata() {
    let h = Full::start();
    let (_, _, clips) = fixture(&h);
    for revision in Revision::ALL {
        let agent = Agent::connect(&h, Profile::Standard, revision).await;
        let result = agent.call("render_frame", json!({ "t": "2s" })).await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let images: Vec<_> = result
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Image(i) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(images.len(), 1, "{revision:?}");
        assert_eq!(images[0].mime_type, "image/jpeg");
        let (w, hgt) = jpeg_size(&base64_decode(&images[0].data)).expect("a valid JPEG");
        assert!(w.max(hgt) <= 768, "{w}x{hgt}");
        let meta = result.structured_content.unwrap();
        assert!(
            meta.get("data").is_none(),
            "the base64 stays out of the metadata"
        );
        assert_eq!(meta["width"].as_u64(), Some(u64::from(w)));
        assert_eq!(meta["height"].as_u64(), Some(u64::from(hgt)));
        assert_eq!(meta["time"], json!(2 * SEC));
        assert!(meta["rev"].is_u64());
        assert!(meta["notes"].is_array());
        let visible: Vec<&Value> = meta["items_visible"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| &v["item"])
            .collect();
        assert!(
            clips[0]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| visible.contains(&c)),
            "{visible:?} vs {:?}",
            clips[0]
        );
        // A PNG takes a different path through the same tool.
        let png = agent
            .call(
                "render_frame",
                json!({ "t": "2s", "format": "png", "max_size": 256 }),
            )
            .await;
        assert!(
            png.content
                .iter()
                .any(|c| matches!(c, ContentBlock::Image(i) if i.mime_type == "image/png"))
        );
        // cutweave_query returns the same image block.
        let queried = agent
            .call(
                "cutweave_query",
                json!({ "name": "render.frame", "args": { "t": "2s" } }),
            )
            .await;
        assert!(
            queried
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::Image(_)))
        );
        let _ = agent.service.cancel().await;
    }
}

/// A request above 1568 px answers with a link, and the link renders the frame.
#[tokio::test]
async fn a_large_frame_is_a_resource_link_that_reads_as_a_blob() {
    let h = Full::start();
    fixture(&h);
    for revision in Revision::ALL {
        let agent = Agent::connect(&h, Profile::Standard, revision).await;
        let result = agent
            .call("render_frame", json!({ "t": "2s", "max_size": 1600 }))
            .await;
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert!(
            !result
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::Image(_)))
        );
        let meta = result.structured_content.clone().unwrap();
        let link = meta["link"].as_str().unwrap().to_owned();
        assert!(link.starts_with("cutweave://frame/"), "{link}");
        assert!(
            result
                .content
                .iter()
                .any(|c| matches!(c, ContentBlock::ResourceLink(r) if r.uri == link))
        );
        assert!(meta["items_visible"].is_array());
        let read = agent
            .service
            .read_resource(ReadResourceRequestParams::new(link.clone()))
            .await
            .unwrap();
        let ResourceContents::BlobResourceContents {
            blob, mime_type, ..
        } = &read.contents[0]
        else {
            panic!("a blob")
        };
        assert_eq!(mime_type.as_deref(), Some("image/jpeg"));
        let (w, hgt) = jpeg_size(&base64_decode(blob)).expect("a JPEG");
        assert_eq!(u64::from(w), meta["width"].as_u64().unwrap());
        assert_eq!(u64::from(hgt), meta["height"].as_u64().unwrap());
        // After an edit the link is stale.
        let renamed = agent
            .ok(
                "cutweave_execute",
                json!({ "steps": [{ "cmd": "project.rename", "args": { "name": format!("Moved {revision:?}") } }] }),
            )
            .await;
        assert_eq!(renamed["changed"], true);
        let stale = agent
            .service
            .read_resource(ReadResourceRequestParams::new(link))
            .await;
        assert!(stale.is_err(), "{stale:?}");
        let _ = agent.service.cancel().await;
    }
}

#[tokio::test]
async fn resources_list_and_read() {
    let h = Full::start();
    let (sequence, _, _) = fixture(&h);
    let seq = sequence.as_str().unwrap().to_owned();
    for revision in Revision::ALL {
        let agent = Agent::connect(&h, Profile::Standard, revision).await;
        let listed = agent.service.list_resources(None).await.unwrap().resources;
        let uris: Vec<&str> = listed.iter().map(|r| r.uri.as_str()).collect();
        assert!(uris.contains(&"cutweave://project"), "{uris:?}");
        assert!(uris.contains(&"cutweave://catalog/commands"));
        let timeline = format!("cutweave://sequence/{seq}/timeline");
        assert!(uris.contains(&timeline.as_str()), "{uris:?}");
        let templates = agent
            .service
            .list_resource_templates(None)
            .await
            .unwrap()
            .resource_templates;
        assert_eq!(templates.len(), 2);

        let read_json = |uri: String| {
            let service = &agent.service;
            async move {
                let read = service
                    .read_resource(ReadResourceRequestParams::new(uri))
                    .await
                    .unwrap();
                let ResourceContents::TextResourceContents { text, .. } = &read.contents[0] else {
                    panic!("text")
                };
                serde_json::from_str::<Value>(text).unwrap()
            }
        };
        let project = read_json("cutweave://project".into()).await;
        assert_eq!(project["name"], "Frames");
        assert!(project["rev"].is_u64());
        assert_eq!(project["sequences"][0]["id"], json!(seq));
        let page = read_json(timeline.clone()).await;
        assert!(page.to_string().len() <= 4096, "{}", page.to_string().len());
        assert!(page["tracks"][0].is_object());
        assert!(page["items"][0].is_object());
        let commands = read_json("cutweave://catalog/commands".into()).await;
        assert!(
            commands["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["name"] == "clip.split")
        );
        let missing = agent
            .service
            .read_resource(ReadResourceRequestParams::new("cutweave://nope"))
            .await;
        assert!(missing.is_err());
        let _ = agent.service.cancel().await;
    }
}

/// Acceptance 2: under each revision, subscribing to `cutweave://project` and running
/// `clip_split` produces an update notification within 200 ms.
#[tokio::test]
#[allow(deprecated)]
async fn project_updates_reach_subscribers_on_both_revisions() {
    let h = Full::start();
    let (_, _, clips) = fixture(&h);
    let items = [clips[0][0].clone(), clips[1][0].clone()];
    for (n, revision) in Revision::ALL.into_iter().enumerate() {
        let mut agent = Agent::connect(&h, Profile::Standard, revision).await;
        let mut listening = None;
        match revision {
            Revision::Stateful => {
                agent
                    .service
                    .subscribe(SubscribeRequestParams::new("cutweave://project"))
                    .await
                    .unwrap();
            }
            Revision::Stateless => {
                let filter = SubscriptionFilter::builder()
                    .resource_subscription("cutweave://project")
                    .build();
                listening = Some(agent.service.peer().listen(filter).await.unwrap());
            }
        }
        // Split inside the (first) clip: at 1 s, then (the right half) at 3 s.
        let (item, at) = (items[n].clone(), if n == 0 { "1s" } else { "5s" });
        agent
            .ok("clip_split", json!({ "item": item, "at": at }))
            .await;
        // The clock starts when the split is acknowledged: a debug build on a busy machine
        // spends its own time on the call; the notification is due within 200 ms of the commit.
        let started = Instant::now();
        let uri = match &mut listening {
            None => tokio::time::timeout(StdDuration::from_secs(5), agent.updates.recv())
                .await
                .expect("a notification")
                .unwrap(),
            Some(sub) => loop {
                let note = tokio::time::timeout(StdDuration::from_secs(5), sub.next())
                    .await
                    .expect("a notification")
                    .unwrap()
                    .expect("the stream is open");
                if let ServerNotification::ResourceUpdatedNotification(n) = note {
                    break n.params.uri;
                }
            },
        };
        assert_eq!(uri, "cutweave://project", "{revision:?}");
        let took = started.elapsed();
        println!("update notification {revision:?}: {took:?} after clip_split started");
        assert!(
            took < StdDuration::from_millis(200),
            "{revision:?}: {took:?}"
        );
        let _ = agent.service.cancel().await;
    }
}

#[tokio::test]
async fn render_frame_in_a_transaction_is_refused_with_advice() {
    let h = Full::start();
    fixture(&h);
    let agent = Agent::connect(&h, Profile::Standard, Revision::Stateful).await;
    let r = agent
        .call(
            "cutweave_execute",
            json!({ "steps": [{ "cmd": "render.frame", "args": { "t": "1s" } }] }),
        )
        .await;
    assert_eq!(r.is_error, Some(true));
    let s = r.structured_content.unwrap();
    assert_eq!(s["error"]["code"], "unsupported");
    assert!(
        s["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("call render_frame")
    );
    assert_eq!(s["failed_step"], 0);
    let _ = agent.service.cancel().await;
}
