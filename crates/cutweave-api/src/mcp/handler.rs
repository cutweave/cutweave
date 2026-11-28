// SPDX-License-Identifier: GPL-3.0-or-later
//! The rmcp [`ServerHandler`]: a thin adapter from MCP to the engine's dispatch API.
//!
//! One handler serves one MCP session (2025-11-25, `Mcp-Session-Id`) or one request (2026-07-28,
//! stateless). It opens **one engine connection** the first time it needs the engine, for the
//! credential the request was authenticated with (the actor is `agent:<token label>`, derived
//! server-side, ADR-0016); later requests of the session must present the same credential.
//! When the session ends (`DELETE`, idle timeout, server shutdown) the handler and its
//! connection drop, which rolls back any transaction the connection still holds (WP-1B.09).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::http::request::Parts;
use cutweave_commands::{CmdError, RequestId, Rev};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, RequestMetaObject,
        ResourceContents, ServerCapabilities, ServerConfig, SubscribeRequestParams,
        SubscriptionFilter, Tool, UnsubscribeRequestParams,
    },
    service::{NotificationContext, RequestContext, SubscriptionContext},
};
use serde_json::{Value, json};

use crate::{
    creds::Credential,
    mcp::{
        catalog::{self, Catalog, Target},
        dispatch::{Dispatch, DispatchCall, DispatchConn},
        execute, frame,
        notify::{Changed, Notifier, Subscriptions, updated_uris},
        resources::{self, Kind, Uri},
        results::{self, META_EXPECTED_REV, META_REQUEST_ID},
        search,
    },
    server::Authenticated,
};

/// The engine connection of a session, with the credential it was opened for.
type SessionConn = (Credential, Arc<dyn DispatchConn>);

/// One MCP session's handler (see the module docs).
#[derive(Clone)]
pub struct CutweaveMcp {
    dispatch: Arc<dyn Dispatch>,
    catalog: Arc<Catalog>,
    fixed: Option<Credential>,
    conn: Arc<Mutex<Option<SessionConn>>>,
    /// `cutweave_execute` calls of a session run one at a time (a connection holds at most
    /// one open transaction).
    exec: Arc<tokio::sync::Mutex<()>>,
    /// Resource change notifications (shared by every session of the engine).
    notifier: Arc<Notifier>,
    /// This session's `resources/subscribe` URIs (2025-11-25).
    subscriptions: Arc<Subscriptions>,
    /// The notifier's generation when the last `subscriptions/listen` was accepted.
    listen_from: Arc<AtomicU64>,
    /// The notifier's tool-list generation when the session began, or when its last
    /// `subscriptions/listen` was accepted: a change after it is one the client has to hear of.
    tools_from: Arc<AtomicU64>,
}

impl std::fmt::Debug for CutweaveMcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CutweaveMcp")
            .field("profile", &self.catalog.profile())
            .field("fixed", &self.fixed)
            .finish_non_exhaustive()
    }
}

impl CutweaveMcp {
    /// A handler listing `catalog`'s tools over `dispatch`.
    ///
    /// `fixed` is the credential of transports without per-request authentication (stdio,
    /// WP-1J.03: the process boundary is the credential). Over HTTP it is `None`: the
    /// credential comes from the server's bearer-token middleware ([`Authenticated`]), never
    /// from the request body.
    #[must_use]
    pub fn new(
        dispatch: Arc<dyn Dispatch>,
        catalog: Arc<Catalog>,
        fixed: Option<Credential>,
    ) -> CutweaveMcp {
        let notifier = Notifier::new(Arc::clone(&dispatch));
        CutweaveMcp {
            dispatch,
            catalog,
            fixed,
            conn: Arc::default(),
            exec: Arc::default(),
            notifier,
            subscriptions: Arc::default(),
            listen_from: Arc::default(),
            tools_from: Arc::default(),
        }
    }

    /// Uses `notifier` (one per engine, shared by every handler of a mount) for change
    /// notifications.
    #[must_use]
    pub fn with_notifier(mut self, notifier: Arc<Notifier>) -> CutweaveMcp {
        self.tools_from = Arc::new(AtomicU64::new(notifier.tools_generation()));
        self.notifier = notifier;
        self
    }

    /// The notifier of this handler: [`Notifier::tools_changed`] tells its clients that the
    /// tool list changed.
    #[must_use]
    pub fn notifier(&self) -> &Arc<Notifier> {
        &self.notifier
    }

    /// The catalog this handler lists.
    #[must_use]
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    fn credential(&self, ctx: &RequestContext<RoleServer>) -> Result<Credential, McpError> {
        if let Some(parts) = ctx.extensions.get::<Parts>() {
            return parts
                .extensions
                .get::<Authenticated>()
                .map(|auth| auth.credential.clone())
                .ok_or_else(|| McpError::invalid_request("unauthenticated request", None));
        }
        self.fixed
            .clone()
            .ok_or_else(|| McpError::invalid_request("no credential for this transport", None))
    }

    /// The session's engine connection, opened on first use.
    fn connection(
        &self,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<Arc<dyn DispatchConn>, McpError> {
        let credential = self.credential(ctx)?;
        let mut slot = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        match slot.as_ref() {
            Some((owner, conn)) if *owner == credential => Ok(Arc::clone(conn)),
            Some(_) => Err(McpError::invalid_request(
                "this MCP session was opened with another credential",
                None,
            )),
            None => {
                let conn = self
                    .dispatch
                    .connect(&credential)
                    .map_err(|e| McpError::internal_error(e.message(), None))?;
                *slot = Some((credential, Arc::clone(&conn)));
                Ok(conn)
            }
        }
    }

    async fn call_entry(
        &self,
        conn: Arc<dyn DispatchConn>,
        tool: &str,
        name: &'static str,
        args: Value,
        meta: &RequestMetaObject,
    ) -> CallToolResult {
        let Some(entry) = self.catalog.registry().get(name) else {
            return error_result(tool, &CmdError::internal("tool without a command"), None);
        };
        let read_only = entry.meta.annotations.read_only;
        if name == frame::COMMAND {
            return self.render_frame(conn, tool, args, meta).await;
        }
        let mut call = DispatchCall::new(name, args.clone());
        match envelope(meta) {
            Ok((request_id, expected_rev)) => {
                call.request_id = request_id;
                call.expected_rev = expected_rev;
            }
            Err(e) => return error_result(tool, &e, None),
        }
        match blocking(conn, call).await {
            Ok(reply) => {
                let mut output = reply.output;
                results::wrap_untrusted(name, &mut output);
                results::success(
                    results::summary(name, read_only, reply.rev),
                    output,
                    Some((reply.rev, reply.committed_rev)),
                )
            }
            Err(e) => error_result(tool, &e, Some(&args)),
        }
    }

    /// `render_frame`: the image as an image content block (or, for a large request, a resource
    /// link), under the payload cap (see [`frame`]).
    async fn render_frame(
        &self,
        conn: Arc<dyn DispatchConn>,
        tool: &str,
        args: Value,
        meta: &RequestMetaObject,
    ) -> CallToolResult {
        let expected_rev = match envelope(meta) {
            Ok((_, expected_rev)) => expected_rev,
            Err(e) => return error_result(tool, &e, None),
        };
        let link = frame::wants_link(&args);
        let mut run_args = args.clone();
        let mut lowered = None;
        loop {
            let call = DispatchCall::new(frame::COMMAND, run_args.clone()).expecting(expected_rev);
            let reply = match blocking(Arc::clone(&conn), call).await {
                Ok(reply) => reply,
                Err(e) => return error_result(tool, &e, Some(&args)),
            };
            let revs = (reply.rev, reply.committed_rev);
            let mut output = reply.output;
            if link {
                return frame::link(output, &args, revs);
            }
            let jpeg = frame::is_jpeg(&output);
            if let Some(q) = lowered {
                frame::add_note(
                    &mut output,
                    format!("JPEG quality lowered to {q} to keep the result under 1 MB"),
                );
            }
            let size = match frame::inline(output, revs) {
                Ok(result) => return result,
                Err(size) => size,
            };
            let current = run_args.get("quality").and_then(Value::as_u64);
            if let Some(q) = frame::next_quality(current).filter(|_| jpeg) {
                run_args["quality"] = json!(q);
                lowered = Some(q);
                continue;
            }
            let e = CmdError::invalid(
                "/max_size",
                format!(
                    "the image is {size} bytes, over the {} byte result limit; lower max_size{}",
                    frame::MAX_PAYLOAD,
                    if jpeg { "" } else { " or use format jpeg" }
                ),
            );
            return error_result(tool, &e, Some(&args));
        }
    }

    async fn execute(&self, conn: Arc<dyn DispatchConn>, args: Value) -> CallToolResult {
        let batch = match execute::parse(&self.catalog, args) {
            Ok(batch) => batch,
            Err(e) => return execute_error(&e),
        };
        let _one_at_a_time = self.exec.lock().await;
        let run = {
            let batch = batch.clone();
            tokio::task::spawn_blocking(move || execute::run(conn.as_ref(), &batch)).await
        };
        match run {
            Ok(Ok(mut out)) => {
                if let Some(steps) = out.get_mut("steps").and_then(Value::as_array_mut) {
                    for (step, result) in batch.steps.iter().zip(steps.iter_mut()) {
                        if let Some(output) = result.get_mut("output") {
                            results::wrap_untrusted(step.name, output);
                        }
                    }
                }
                let rev = out["rev"].as_u64().unwrap_or_default();
                let n = batch.steps.len();
                let mut summary = if batch.dry_run {
                    format!(
                        "dry run: {n} step(s) ran and were rolled back; nothing changed (rev {rev})"
                    )
                } else if out.get("changed").is_some() {
                    format!("{n} step(s) applied as one undo step; document at rev {rev}")
                } else {
                    format!("{}: done; document at rev {rev}", batch.steps[0].name)
                };
                if let Some(cp) = out.get("checkpoint").and_then(Value::as_str) {
                    summary.push_str("; checkpoint ");
                    summary.push_str(cp);
                    summary.push_str(" saved before it (edit.restore reverts to it)");
                }
                results::success(summary, out, None)
            }
            Ok(Err(e)) => execute_error(&e),
            Err(join) => execute_error(&execute::ExecError {
                error: CmdError::internal(format!("cutweave_execute failed: {join}")),
                failed_step: None,
                args: None,
                hint: None,
            }),
        }
    }

    async fn query(
        &self,
        conn: Arc<dyn DispatchConn>,
        args: &Value,
        meta: &RequestMetaObject,
    ) -> CallToolResult {
        let tool = catalog::QUERY;
        if let Some(key) = args
            .as_object()
            .and_then(|m| m.keys().find(|k| !matches!(k.as_str(), "name" | "args")))
        {
            let e = CmdError::invalid(
                format!("/{key}"),
                "unknown field; expected `name` and `args`",
            );
            return error_result(tool, &e, None);
        }
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return error_result(
                tool,
                &CmdError::invalid("/name", "expected a query name"),
                None,
            );
        };
        let Some(entry) = self.catalog.reachable(name) else {
            let e = CmdError::not_found(cutweave_commands::ResourceKind::Command, name);
            return error_result(tool, &e, None);
        };
        if !entry.meta.annotations.read_only {
            let e = CmdError::invalid(
                "/name",
                format!(
                    "`{}` changes state; run it with {}",
                    entry.meta.name,
                    catalog::how_to_call(&self.catalog, &entry.meta)
                ),
            );
            return error_result(tool, &e, None);
        }
        let params = match args.get("args") {
            None | Some(Value::Null) => json!({}),
            Some(Value::Object(m)) => Value::Object(m.clone()),
            Some(_) => {
                let e = CmdError::invalid("/args", "expected an object of params");
                return error_result(tool, &e, None);
            }
        };
        let name = entry.meta.name;
        let mut result = self.call_entry(conn, tool, name, params, meta).await;
        // Errors point into `args`.
        if result.is_error == Some(true)
            && let Some(path) = result
                .structured_content
                .as_mut()
                .and_then(|s| s.pointer_mut("/error/path"))
            && let Some(p) = path.as_str()
        {
            *path = json!(format!("/args{p}"));
            if let Some(s) = &result.structured_content {
                let text = s.to_string();
                if let Some(last) = result.content.last_mut() {
                    *last = rmcp::model::ContentBlock::text(text);
                }
            }
        }
        result
    }

    fn search(&self, args: &Value) -> CallToolResult {
        let detail = match args.get("detail") {
            None | Some(Value::Null) => Some(search::Detail::default()),
            Some(value) => value.as_str().and_then(search::Detail::parse),
        };
        let Some(detail) = detail else {
            let e = CmdError::invalid(
                "/detail",
                format!("expected one of {}", search::Detail::NAMES.join(", ")),
            );
            return error_result(catalog::SEARCH, &e, None);
        };
        let limit = args["limit"]
            .as_u64()
            .and_then(|l| usize::try_from(l).ok())
            .unwrap_or(search::DEFAULT_LIMIT);
        let found = search::find(
            &self.catalog,
            args["query"].as_str().unwrap_or_default(),
            detail.limit(limit),
        );
        let (total, query) = (found.total, found.query);
        let matches: Vec<Value> = found
            .entries
            .iter()
            .map(|e| match detail {
                search::Detail::Names => json!(e.meta.name),
                search::Detail::Summary => catalog::entry_summary(&self.catalog, &e.meta),
                search::Detail::Schema => {
                    let mut hit = catalog::entry_summary(&self.catalog, &e.meta);
                    hit["input_schema"] = e.meta.input_schema.clone();
                    hit
                }
            })
            .collect();
        let mut out = json!({ "matches": matches, "total": total });
        if total == 0 {
            let mut namespaces: Vec<&str> = self
                .catalog
                .reachable_entries()
                .map(|e| e.meta.namespace())
                .collect();
            namespaces.dedup();
            out["hint"] = json!(format!(
                "no match; try one word or a namespace: {}",
                namespaces.join(", ")
            ));
        }
        let summary = format!("{total} match(es) for \"{query}\"");
        results::success(summary, out, None)
    }

    /// What `cutweave_describe_command` says about one command.
    fn description(&self, meta: &cutweave_commands::CommandMeta) -> Value {
        let mut out = catalog::entry_summary(&self.catalog, meta);
        if let Some(doc) = meta.input_schema.get("description") {
            out["description"] = doc.clone();
        }
        out["undoable"] = json!(meta.undoable);
        out["annotations"] = json!(meta.annotations);
        out["input_schema"] = meta.input_schema.clone();
        out["output_schema"] = results::mcp_output_schema(meta.name, &meta.output_schema);
        out
    }

    fn describe(&self, args: &Value) -> CallToolResult {
        let tool = catalog::DESCRIBE;
        let given = |key: &str| args.get(key).filter(|v| !v.is_null());
        let names = match (given("name"), given("names")) {
            (Some(_), Some(_)) => {
                let e = CmdError::invalid("/names", "give `name` or `names`, not both");
                return error_result(tool, &e, None);
            }
            (_, Some(names)) => names,
            // One command: the result is the description itself, as before `names` existed.
            (_, None) => {
                let name = args["name"].as_str().unwrap_or_default();
                let Some(entry) = self.catalog.reachable(name) else {
                    let e = CmdError::not_found(cutweave_commands::ResourceKind::Command, name);
                    return error_result(tool, &e, None);
                };
                let meta = &entry.meta;
                return results::success(
                    format!("{}: {}", meta.name, meta.summary),
                    self.description(meta),
                    None,
                );
            }
        };
        let names = match names.as_array() {
            Some(list) if (1..=search::MAX_DESCRIBED).contains(&list.len()) => list,
            _ => {
                let e = CmdError::invalid(
                    "/names",
                    format!("expected 1 to {} command names", search::MAX_DESCRIBED),
                );
                return error_result(tool, &e, None);
            }
        };
        let mut commands = Vec::new();
        let mut listed = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let Some(name) = name.as_str() else {
                let e = CmdError::invalid(format!("/names/{i}"), "expected a command name");
                return error_result(tool, &e, None);
            };
            let Some(entry) = self.catalog.reachable(name) else {
                let e = CmdError::not_found(cutweave_commands::ResourceKind::Command, name);
                return error_result(tool, &e, None);
            };
            listed.push(entry.meta.name);
            commands.push(self.description(&entry.meta));
        }
        results::success(
            format!("{} command(s): {}", listed.len(), listed.join(", ")),
            json!({ "commands": commands }),
            None,
        )
    }
}

/// The envelope a core tool call may carry in request `_meta`: an idempotency key and an
/// expected revision (models cannot set `_meta`; programmatic clients and proxies can).
fn envelope(meta: &RequestMetaObject) -> Result<(Option<RequestId>, Option<Rev>), CmdError> {
    let request_id = match meta.get(META_REQUEST_ID) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            Some(s.parse().map_err(|e: cutweave_commands::RequestIdError| {
                CmdError::invalid("/_meta/io.cutweave~1request_id", e.to_string())
            })?)
        }
        Some(_) => {
            return Err(CmdError::invalid(
                "/_meta/io.cutweave~1request_id",
                "expected a UUID string",
            ));
        }
    };
    let expected_rev = match meta.get(META_EXPECTED_REV) {
        None | Some(Value::Null) => None,
        Some(v) => Some(Rev(v.as_u64().ok_or_else(|| {
            CmdError::invalid(
                "/_meta/io.cutweave~1expected_rev",
                "expected a non-negative integer revision",
            )
        })?)),
    };
    Ok((request_id, expected_rev))
}

/// Runs one blocking engine call on the blocking pool.
async fn blocking(
    conn: Arc<dyn DispatchConn>,
    call: DispatchCall,
) -> Result<crate::mcp::dispatch::DispatchReply, CmdError> {
    tokio::task::spawn_blocking(move || conn.call(call))
        .await
        .map_err(|e| CmdError::internal(format!("the request task failed: {e}")))?
}

fn error_result(tool: &str, error: &CmdError, args: Option<&Value>) -> CallToolResult {
    results::failure(tool, results::error_json(error, args, ""), &[])
}

fn execute_error(e: &execute::ExecError) -> CallToolResult {
    let prefix = e
        .failed_step
        .map_or_else(String::new, |i| format!("/steps/{i}/args"));
    let mut error = results::error_json(&e.error, e.args.as_ref(), &prefix);
    if let Some(hint) = e.hint {
        error["hint"] = json!(hint);
    }
    let failed_step = e.failed_step.map_or(Value::Null, |i| json!(i));
    results::failure(
        catalog::EXECUTE,
        error,
        &[("failed_step", failed_step), ("rolled_back", json!(true))],
    )
}

/// A resource error for a failed engine call.
fn resource_error(uri: &str, e: &CmdError) -> McpError {
    let data = Some(json!({ "uri": uri, "error": results::error_json(e, None, "") }));
    match e {
        CmdError::NotFound { .. } => McpError::resource_not_found(e.message(), data),
        CmdError::Invalid { .. } | CmdError::Conflict { .. } => {
            McpError::invalid_params(e.message(), data)
        }
        _ => McpError::internal_error(e.message(), data),
    }
}

fn json_contents(uri: &str, value: &Value) -> ResourceContents {
    ResourceContents::text(value.to_string(), uri).with_mime_type(resources::JSON)
}

impl CutweaveMcp {
    /// One engine call for resource `uri`.
    async fn engine(
        &self,
        ctx: &RequestContext<RoleServer>,
        uri: &str,
        name: &'static str,
        params: Value,
    ) -> Result<crate::mcp::dispatch::DispatchReply, McpError> {
        let conn = self.connection(ctx)?;
        blocking(conn, DispatchCall::new(name, params))
            .await
            .map_err(|e| resource_error(uri, &e))
    }

    async fn read(
        &self,
        uri: &str,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<ResourceContents, McpError> {
        let parsed = resources::parse(uri)
            .map_err(|m| McpError::resource_not_found(m, Some(json!({ "uri": uri }))))?;
        match parsed {
            Uri::Commands => Ok(json_contents(uri, &resources::commands(&self.catalog))),
            Uri::Project => {
                let reply = self.engine(ctx, uri, "project.info", json!({})).await?;
                let mut out = reply.output;
                out["rev"] = json!(reply.rev);
                Ok(json_contents(uri, &out))
            }
            Uri::Timeline { sequence, cursor } => {
                let reply = self
                    .engine(
                        ctx,
                        uri,
                        "timeline.get",
                        json!({ "sequence": sequence, "depth": 2 }),
                    )
                    .await?;
                let page = resources::timeline_page(&reply.output, &sequence, reply.rev.0, cursor)
                    .map_err(|m| McpError::invalid_params(m, Some(json!({ "uri": uri }))))?;
                Ok(json_contents(uri, &page))
            }
            Uri::Frame(f) => {
                let mut params = json!({ "t": f.t });
                if let Some(s) = &f.sequence {
                    params["sequence"] = json!(s);
                }
                if let Some(m) = f.max_size {
                    params["max_size"] = json!(m);
                }
                if let Some(format) = &f.format {
                    params["format"] = json!(format);
                }
                if let Some(q) = f.quality {
                    params["quality"] = json!(q);
                }
                let reply = self.engine(ctx, uri, frame::COMMAND, params).await?;
                if reply.output["rev"].as_u64() != Some(f.rev) {
                    let e = CmdError::invalid(
                        "/uri",
                        format!(
                            "this frame link is for document revision {}, which is gone (the \
                             document is at {}); call render_frame again",
                            f.rev, reply.rev
                        ),
                    );
                    return Err(resource_error(uri, &e));
                }
                let data = reply.output["data"].as_str().unwrap_or_default();
                let mime = reply.output["mime"].as_str().unwrap_or("image/jpeg");
                Ok(ResourceContents::blob(data, uri).with_mime_type(mime))
            }
        }
    }
}

/// Whether `uri` is a resource a client can subscribe to.
fn subscribable(uri: &str) -> bool {
    matches!(
        resources::kind(uri),
        Some(Kind::Project | Kind::Timeline | Kind::Commands)
    )
}

// rmcp handler methods are `async fn` by convention even where nothing is awaited.
#[allow(clippy::unused_async_trait_impl, deprecated)]
impl ServerHandler for CutweaveMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                // Sent only when the set of commands itself changes (a plugin loads): WP-2I.05.
                .enable_tool_list_changed()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
        )
        .with_server_info(Implementation::new("cutweave", env!("CARGO_PKG_VERSION")))
        .with_instructions(self.catalog.instructions())
    }

    /// 2025-11-25 and older: the session is up, so tool-list changes can reach its peer
    /// (2026-07-28 has no `initialized`; a client there asks with `subscriptions/listen`).
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.subscriptions.watch_tools(
            &self.notifier,
            &context.peer,
            self.tools_from.load(Ordering::SeqCst),
        );
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

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.catalog.tool(name).cloned()
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut listed = resources::fixed();
        let conn = self.connection(&ctx)?;
        if let Ok(info) = blocking(conn, DispatchCall::new("project.info", json!({}))).await {
            listed.extend(
                resources::sequences(&info.output)
                    .iter()
                    .map(|(id, name)| resources::timeline_resource(id, name)),
            );
        }
        Ok(ListResourcesResult::with_all_items(listed))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(
            resources::templates(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let contents = self.read(&request.uri, &ctx).await?;
        Ok(ReadResourceResult::new(vec![contents]).into())
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        if !subscribable(&request.uri) {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` cannot be subscribed to; subscribe to cutweave://project or a \
                     sequence timeline",
                    request.uri
                ),
                None,
            ));
        }
        self.subscriptions
            .add(request.uri, &self.notifier, &ctx.peer);
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.subscriptions.remove(&request.uri);
        Ok(())
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        // Subscribe to the engine's events before the acknowledgment goes out, and remember
        // where we are: `listen` replays a change that lands in between.
        self.notifier.start();
        self.listen_from
            .store(self.notifier.generation(), Ordering::SeqCst);
        self.tools_from
            .store(self.notifier.tools_generation(), Ordering::SeqCst);
        let uris: Vec<String> = requested
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|u| subscribable(u))
            .cloned()
            .collect();
        let filter = SubscriptionFilter::builder().resource_subscriptions(uris);
        // rmcp keeps this only when the client asked for it.
        Some(filter.tools_list_changed().build())
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        use tokio::sync::broadcast::error::RecvError;
        let uris = context
            .accepted()
            .resource_subscriptions
            .clone()
            .unwrap_or_default();
        let tools = context.accepted().tools_list_changed == Some(true);
        if uris.is_empty() && !tools {
            context.cancelled().await;
            return Ok(());
        }
        let mut documents = self.notifier.subscribe();
        let mut lists = self.notifier.subscribe_tools();
        // A change that landed between the acknowledgment and here is replayed.
        let mut changed = (self.notifier.generation() > self.listen_from.load(Ordering::SeqCst))
            .then_some(Changed::ALL);
        let mut listed =
            tools && self.notifier.tools_generation() > self.tools_from.load(Ordering::SeqCst);
        loop {
            if std::mem::take(&mut listed)
                && context.sink().notify_tool_list_changed().await.is_err()
            {
                return Ok(());
            }
            for uri in updated_uris(changed.take().unwrap_or_default(), &uris) {
                if context.sink().notify_resource_updated(uri).await.is_err() {
                    return Ok(());
                }
            }
            tokio::select! {
                () = context.cancelled() => return Ok(()),
                received = documents.recv() => match received {
                    Ok(next) => changed = Some(next),
                    Err(RecvError::Lagged(_)) => changed = Some(Changed::ALL),
                    Err(RecvError::Closed) => break,
                },
                received = lists.recv() => match received {
                    // Several changes this listener fell behind on are one "the list changed".
                    Ok(()) | Err(RecvError::Lagged(_)) => listed = tools,
                    Err(RecvError::Closed) => break,
                },
            }
        }
        context.cancelled().await;
        Ok(())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(target) = self.catalog.target(&request.name) else {
            return Err(McpError::invalid_params(
                format!(
                    "unknown tool `{}`; cutweave_search_commands finds every command",
                    request.name
                ),
                None,
            ));
        };
        let args = Value::Object(request.arguments.clone().unwrap_or_default());
        let meta = request.meta.clone().unwrap_or_else(|| ctx.meta.clone());
        let result = match target {
            Target::Search => self.search(&args),
            Target::Describe => self.describe(&args),
            Target::Entry(name) => {
                let conn = self.connection(&ctx)?;
                self.call_entry(conn, &request.name, name, args, &meta)
                    .await
            }
            Target::Execute => {
                let conn = self.connection(&ctx)?;
                self.execute(conn, args).await
            }
            Target::Query => {
                let conn = self.connection(&ctx)?;
                self.query(conn, &args, &meta).await
            }
        };
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_reads_request_id_and_expected_rev() {
        let mut meta = RequestMetaObject::default();
        assert_eq!(envelope(&meta), Ok((None, None)));
        meta.insert(
            META_REQUEST_ID.into(),
            json!("0190f5c2-a1b2-7c3d-8e4f-5a6b7c8d9e0f"),
        );
        meta.insert(META_EXPECTED_REV.into(), json!(4));
        let (id, rev) = envelope(&meta).unwrap();
        assert!(id.is_some());
        assert_eq!(rev, Some(Rev(4)));
        meta.insert(META_EXPECTED_REV.into(), json!("four"));
        assert_eq!(envelope(&meta).unwrap_err().code(), "invalid");
    }
}
