// SPDX-License-Identifier: GPL-3.0-or-later
//! The rmcp [`ServerHandler`]: a thin adapter from MCP to the engine (no business logic).
//!
//! - `tools/list`: the endpoint's [`Catalog`].
//! - `tools/call`: entry tools and `cutweave_query` go through `Engine::invoke`,
//!   `cutweave_execute` through `Engine::execute` (one `Tx`, one undo entry, `$ref`). Command
//!   errors are tool results with `isError: true` and the typed error as JSON, so the model can
//!   correct itself; only unknown tools are protocol errors.
//! - Envelope controls for entry tools travel in request `_meta` (`io.cutweave/request_id`,
//!   `io.cutweave/expected_rev`, `io.cutweave/dry_run`), keeping `inputSchema` identical to the
//!   RPC params schema. `cutweave_execute` also takes them as arguments.
//! - Resource `cutweave://project` (summary + rev) with change notifications for both revisions
//!   (see [`crate::notifier`]).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, DiscoverResult,
        Implementation, InitializeRequestParams, InitializeResult, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, RequestMetaObject, Resource, ResourceContents, ServerCapabilities,
        ServerConfig, SubscribeRequestParams, SubscriptionFilter, UnsubscribeRequestParams,
    },
    service::{RequestContext, SubscriptionContext},
};
use s5_bus::{
    Caller, CmdError, CommandEntry, EntityKind, EntryKind, Rev,
    engine::{Batch, Request, Step},
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::{
    EngineHandle,
    catalog::{self, Catalog, ToolTarget},
    http::AgentIdentity,
    notifier::{ChangeSink, DocChange, forward},
};

/// The one resource: project summary + revision. Subscribe to it for change notifications.
pub const PROJECT_URI: &str = "cutweave://project";

/// Short server instructions (they count against the client's context, like tool definitions).
const INSTRUCTIONS: &str = "Cutweave video editor (spike S5). Look before you edit: timeline_get \
returns tracks, items and media by id; render_frame returns a frame as an image. Address \
everything by id. Times are integer flicks (705600000 per second) or strings like \"2.5s\" or \
\"00:00:02:00\". Every edit is undoable with edit_undo. Commands not listed as tools: \
cutweave_search_commands, then cutweave_describe_command, then cutweave_execute or \
cutweave_query.";

const META_REQUEST_ID: &str = "io.cutweave/request_id";
const META_EXPECTED_REV: &str = "io.cutweave/expected_rev";
const META_DRY_RUN: &str = "io.cutweave/dry_run";

/// One MCP connection's handler. Cheap to clone; HTTP creates one per session (2025-11-25) or
/// per request (2026-07-28), all sharing the same engine and catalog.
#[derive(Clone)]
pub struct CutweaveMcp {
    engine: EngineHandle,
    catalog: Arc<Catalog>,
    fixed_caller: Option<Caller>,
    legacy_subscriptions: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pending_listen: Arc<Mutex<Option<broadcast::Receiver<DocChange>>>>,
}

impl CutweaveMcp {
    /// A handler over `engine` listing `catalog`'s tools.
    ///
    /// `fixed_caller` is the actor for transports without per-request credentials (stdio: the
    /// process boundary is the credential). HTTP passes `None`; the caller then comes from the
    /// [`AgentIdentity`] the auth middleware derived from the bearer token.
    #[must_use]
    pub fn new(engine: EngineHandle, catalog: Arc<Catalog>, fixed_caller: Option<Caller>) -> Self {
        CutweaveMcp {
            engine,
            catalog,
            fixed_caller,
            legacy_subscriptions: Arc::default(),
            pending_listen: Arc::default(),
        }
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    fn caller(&self, ctx: &RequestContext<RoleServer>) -> Result<Caller, McpError> {
        if let Some(parts) = ctx.extensions.get::<axum::http::request::Parts>() {
            // Never trust anything in the request body for the actor: only the middleware's
            // token -> identity mapping.
            return parts
                .extensions
                .get::<AgentIdentity>()
                .map(|id| Caller::agent(&id.label))
                .ok_or_else(|| McpError::invalid_request("unauthenticated request", None));
        }
        self.fixed_caller
            .clone()
            .ok_or_else(|| McpError::invalid_request("no credential for this transport", None))
    }

    fn call_entry(
        &self,
        caller: &Caller,
        entry: &'static CommandEntry,
        params: Value,
        envelope: Envelope,
    ) -> CallToolResult {
        let request = Request {
            name: entry.meta.name.to_owned(),
            params,
            request_id: envelope.request_id,
            expected_rev: envelope.expected_rev,
            dry_run: envelope.dry_run,
        };
        match self.engine.with(|e| e.invoke(caller, request)) {
            Ok(resp) => {
                let mut result = resp.result;
                let images = take_image(entry, &mut result);
                let mut structured = json!({ "rev": resp.rev, "result": result });
                if resp.dry_run {
                    structured["dry_run"] = json!(true);
                }
                if resp.deduplicated {
                    structured["deduplicated"] = json!(true);
                }
                success(structured, images)
            }
            Err(e) => failure(&e),
        }
    }

    fn search(&self, args: &Value) -> CallToolResult {
        let query = args["query"].as_str().unwrap_or_default().to_lowercase();
        let limit = args["limit"]
            .as_u64()
            .and_then(|l| usize::try_from(l).ok())
            .unwrap_or(20)
            .clamp(1, 50);
        let words: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let mut hits: Vec<(usize, &'static CommandEntry)> = self
            .catalog
            .all_reachable()
            .filter_map(|e| {
                let hay = format!("{} {}", e.meta.name, e.meta.summary).to_lowercase();
                let score = words.iter().filter(|w| hay.contains(**w)).count();
                (words.is_empty() || score > 0).then_some((score, e))
            })
            .collect();
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.meta.name.cmp(b.1.meta.name)));
        let total = hits.len();
        let matches: Vec<Value> = hits
            .into_iter()
            .take(limit)
            .map(|(_, e)| catalog::entry_summary(&self.catalog, e))
            .collect();
        let mut out = json!({ "matches": matches, "total": total });
        if total == 0 {
            let mut namespaces: Vec<&str> = self
                .catalog
                .all_reachable()
                .filter_map(|e| e.meta.name.split_once('.').map(|(ns, _)| ns))
                .collect();
            namespaces.dedup();
            out["hint"] = json!(format!(
                "no match; try one word, or a namespace: {}",
                namespaces.join(", ")
            ));
        }
        success(out, Vec::new())
    }

    fn describe(&self, args: &Value) -> CallToolResult {
        let name = args["name"].as_str().unwrap_or_default();
        let Some(entry) = self.catalog.reachable(name) else {
            return failure(&unknown_command(name));
        };
        let mut out = catalog::entry_summary(&self.catalog, entry);
        out["call"] = json!(catalog::how_to_call(&self.catalog, entry));
        out["undoable"] = json!(entry.meta.undoable);
        out["annotations"] = json!(entry.meta.annotations);
        out["params_schema"] = entry.params_schema();
        out["output_schema"] = entry.output_schema();
        success(out, Vec::new())
    }

    fn execute(&self, caller: &Caller, args: Value) -> CallToolResult {
        let mut batch: Batch = match serde_json::from_value(args) {
            Ok(b) => b,
            Err(e) => return failure(&CmdError::invalid("", e.to_string())),
        };
        let mut entries = Vec::with_capacity(batch.steps.len());
        for (i, step) in batch.steps.iter_mut().enumerate() {
            let Some(entry) = self.catalog.reachable(&step.name) else {
                return failure(&unknown_command(&step.name).under(&format!("/steps/{i}")));
            };
            step.name = entry.meta.name.to_owned(); // accept `clip_split` as well as `clip.split`
            entries.push(entry);
        }
        // Engine ops (edit.redo) and non-undoable commands (project.open) cannot join a Tx, but
        // must stay reachable for the long tail: a single such step runs as a plain call.
        let outcome = match (batch.steps.as_slice(), entries.as_slice()) {
            ([step], [entry])
                if entry.is_engine_op()
                    || (entry.meta.kind == EntryKind::Command && !entry.meta.undoable) =>
            {
                let Step { name, params } = step.clone();
                let request = Request {
                    name,
                    params,
                    request_id: batch.request_id.take(),
                    expected_rev: batch.expected_rev,
                    dry_run: batch.dry_run,
                };
                self.engine
                    .with(|e| e.invoke(caller, request))
                    .map(|r| (r.rev, vec![r.result], r.dry_run, r.deduplicated))
            }
            _ => self
                .engine
                .with(|e| e.execute(caller, batch))
                .map(|r| (r.rev, r.results, r.dry_run, r.deduplicated)),
        };
        match outcome {
            Ok((rev, mut results, dry_run, deduplicated)) => {
                let mut images = Vec::new();
                for (entry, result) in entries.iter().zip(results.iter_mut()) {
                    images.extend(take_image(entry, result));
                }
                let mut out = json!({ "rev": rev, "results": results });
                if dry_run {
                    out["dry_run"] = json!(true);
                }
                if deduplicated {
                    out["deduplicated"] = json!(true);
                }
                success(out, images)
            }
            Err(e) => failure(&e),
        }
    }

    fn query(&self, caller: &Caller, args: &Value, envelope: Envelope) -> CallToolResult {
        let name = args["name"].as_str().unwrap_or_default();
        let Some(entry) = self.catalog.reachable(name) else {
            return failure(&unknown_command(name));
        };
        if entry.meta.kind != EntryKind::Query {
            return failure(&CmdError::invalid(
                "/name",
                format!(
                    "`{}` changes the document; use cutweave_execute",
                    entry.meta.name
                ),
            ));
        }
        let params = args.get("args").cloned().unwrap_or(Value::Null);
        self.call_entry(caller, entry, params, envelope)
    }

    fn project_summary(&self) -> Value {
        self.engine.with(|e| {
            let doc = e.doc();
            json!({
                "project": doc.meta.name,
                "rev": e.rev(),
                "sequence": doc.sequence.id,
                "frame_rate": doc.sequence.frame_rate,
                "duration": doc.duration(),
                "tracks": doc.tracks.len(),
                "items": doc.items.len(),
                "media": doc.media.len(),
            })
        })
    }
}

/// Envelope controls taken from request `_meta`.
#[derive(Default)]
struct Envelope {
    request_id: Option<String>,
    expected_rev: Option<Rev>,
    dry_run: bool,
}

impl Envelope {
    fn from_meta(meta: &RequestMetaObject) -> Envelope {
        Envelope {
            request_id: meta
                .get(META_REQUEST_ID)
                .and_then(Value::as_str)
                .map(str::to_owned),
            expected_rev: meta.get(META_EXPECTED_REV).and_then(Value::as_u64).map(Rev),
            dry_run: meta
                .get(META_DRY_RUN)
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

fn unknown_command(name: &str) -> CmdError {
    CmdError::NotFound {
        entity: EntityKind::Command,
        id: name.to_owned(),
    }
}

/// Moves an image output's base64 `data` out of `result` (see [`catalog::has_image_output`]).
fn take_image(entry: &CommandEntry, result: &mut Value) -> Vec<(String, String)> {
    if !catalog::has_image_output(entry) {
        return Vec::new();
    }
    let mime = result["mime"].as_str().map(str::to_owned);
    match (mime, result.as_object_mut().and_then(|m| m.remove("data"))) {
        (Some(mime), Some(Value::String(data))) => vec![(data, mime)],
        _ => Vec::new(),
    }
}

/// Success: images first, then the structured result as JSON text (for clients and models that
/// ignore `structuredContent`), plus `structuredContent`.
fn success(structured: Value, images: Vec<(String, String)>) -> CallToolResult {
    let mut result = CallToolResult::structured(structured);
    let mut content: Vec<ContentBlock> = images
        .into_iter()
        .map(|(data, mime)| ContentBlock::image(data, mime))
        .collect();
    content.append(&mut result.content);
    result.content = content;
    result
}

/// A command error as a tool result the model can read and act on.
fn failure(error: &CmdError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(
        json!({ "error": error.to_wire() }).to_string(),
    )])
}

// rmcp handler methods are written as `async fn` (the rmcp convention) even when they do not
// await; the engine calls are synchronous and short.
#[allow(clippy::unused_async_trait_impl)]
impl ServerHandler for CutweaveMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_server_info(Implementation::new(
            "cutweave-s5",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    // Lifecycle logging only (the negotiation itself is rmcp's): which revision each client
    // ends up on is the first thing WP-0H.03 records per agent host.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        ctx.peer.set_peer_info(request.clone());
        let result = self.negotiate_initialize(&request);
        if let Ok(r) = &result {
            tracing::info!(
                client = %request.client_info.name,
                client_version = %request.client_info.version,
                requested = %request.protocol_version,
                negotiated = %r.protocol_version,
                profile = self.catalog.profile().as_str(),
                "initialize (2025-11-25-style session lifecycle)"
            );
        }
        result
    }

    async fn discover(&self, ctx: RequestContext<RoleServer>) -> Result<DiscoverResult, McpError> {
        tracing::info!(
            client = ?ctx.meta.client_info().map(|c| format!("{} {}", c.name, c.version)),
            requested = ?ctx.meta.protocol_version().map(|v| v.to_string()),
            profile = self.catalog.profile().as_str(),
            "server/discover (2026-07-28 stateless lifecycle)"
        );
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            self.catalog.tools().to_vec(),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.catalog
            .tools()
            .iter()
            .find(|t| t.name == name)
            .cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let caller = self.caller(&ctx)?;
        let Some(target) = self.catalog.target(&request.name) else {
            return Err(McpError::invalid_params(
                format!(
                    "unknown tool `{}`; cutweave_search_commands finds every command",
                    request.name
                ),
                None,
            ));
        };
        let envelope = Envelope::from_meta(request.meta.as_ref().unwrap_or(&ctx.meta));
        let args = Value::Object(request.arguments.unwrap_or_default());
        let result = match target {
            ToolTarget::Entry(entry) => self.call_entry(&caller, entry, args, envelope),
            ToolTarget::Search => self.search(&args),
            ToolTarget::Describe => self.describe(&args),
            ToolTarget::Execute => self.execute(&caller, args),
            ToolTarget::Query => self.query(&caller, &args, envelope),
        };
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(PROJECT_URI, "project")
                .with_description(
                    "Project summary and revision; subscribe for change notifications",
                )
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        if request.uri != PROJECT_URI {
            return Err(McpError::resource_not_found(
                format!("no resource `{}`", request.uri),
                None,
            ));
        }
        let text = self.project_summary().to_string();
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, PROJECT_URI).with_mime_type("application/json"),
        ])
        .into())
    }

    // 2025-11-25 (and older): resources/subscribe + notifications/resources/updated.
    #[allow(deprecated)]
    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if request.uri != PROJECT_URI {
            return Err(McpError::resource_not_found(
                format!("no resource `{}`", request.uri),
                None,
            ));
        }
        // Subscribe to the feed before acknowledging so no change after the ack is missed.
        let changes = self.engine.notifier().subscribe();
        let stop = CancellationToken::new();
        if let Some(old) = self
            .legacy_subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request.uri.clone(), stop.clone())
        {
            old.cancel();
        }
        let sink = ChangeSink::Legacy(ctx.peer.clone());
        tokio::spawn(async move {
            forward(changes, sink, vec![request.uri], stop.cancelled_owned()).await;
        });
        Ok(())
    }

    #[allow(deprecated)]
    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if let Some(stop) = self
            .legacy_subscriptions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&request.uri)
        {
            stop.cancel();
        }
        Ok(())
    }

    // 2026-07-28: subscriptions/listen. rmcp sends the acknowledgement *before* calling `listen`,
    // so the feed is joined here (called just before the ack) to avoid losing a change made
    // between the ack and the start of `listen`.
    fn accepted_subscription_filter(
        &self,
        _requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        *self
            .pending_listen
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(self.engine.notifier().subscribe());
        Some(
            SubscriptionFilter::builder()
                .resource_subscription(PROJECT_URI)
                .build(),
        )
    }

    async fn listen(&self, ctx: SubscriptionContext) -> Result<(), McpError> {
        let changes = self
            .pending_listen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| self.engine.notifier().subscribe());
        let uris = ctx
            .accepted()
            .resource_subscriptions
            .clone()
            .unwrap_or_default();
        if uris.is_empty() {
            ctx.cancelled().await;
            return Ok(());
        }
        let sink = ChangeSink::Modern(ctx.sink().clone());
        forward(changes, sink, uris, ctx.cancelled()).await;
        Ok(())
    }
}
