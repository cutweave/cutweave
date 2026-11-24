// SPDX-License-Identifier: GPL-3.0-or-later
//! What agents may reach and who is connected (WP-1J.05; arch §6, §14, §18, ADR-0016).
//!
//! - [`ProjectSetAllowedRoots`] (`project.set_allowed_roots`): the desktop app applies its
//!   "Folders agents can use" setting to its engine. UI credential only (ADR-0016 lists
//!   "changing filesystem allowlists"), not undoable, not journaled, never dirties the project.
//!   The folders replace the engine's approved roots (the `--allow-root` ones included) and are
//!   read on every call that takes a path, so a removed folder stops access at once, also for
//!   connections that are already open.
//! - [`ProjectConnections`] (`project.connections`): the agent connections open on the engine
//!   (actor, connected since, last command and when), read-only. [`Connections`] keeps them;
//!   `project.connection_opened` and `project.connection_closed` announce the changes.
//!
//! Folders are canonicalized before they are stored: the path must be absolute, without `..`,
//! and name an existing folder; symlinks and junctions are resolved, and on Windows the `\\?\`
//! prefix `canonicalize` adds is dropped again where the plain form names the same folder.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use cutweave_commands::{
    Actor, ActorCtx, Annotations, CmdError, CommandSpec, ConnId, Described, EngineCommand,
    EntryKind, McpExposure, Registrable, Registry, RegistryError, Scope, WallTime, command, wire,
};

use crate::events::{EngineEvent, EventBus, SessionEvent};
use crate::session::SessionCall;

/// The most folders `project.set_allowed_roots` takes.
pub const MAX_ALLOWED_ROOTS: usize = 64;
/// The longest folder path it takes, in bytes.
const MAX_ROOT_LEN: usize = 4096;
/// The longest command name recorded as a connection's last command.
const MAX_COMMAND_NAME: usize = 128;

/// Registers `project.set_allowed_roots` and `project.connections`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ProjectSetAllowedRoots>()?
        .register::<ProjectConnections>()?;
    Ok(())
}

// ----- project.set_allowed_roots ---------------------------------------------------------------

/// Result of `project.set_allowed_roots`.
#[wire]
pub struct ProjectSetAllowedRootsOutput {
    /// The approved folders now in force: canonical (symlinks resolved), in the order given,
    /// without duplicates.
    pub roots: Vec<String>,
}

/// Sets the folders agents and other non-UI callers may use (import media from, open projects
/// in, save projects and exports into) besides the open project's folder: the desktop app's
/// Settings > Agents and MCP. The list replaces the engine's approved folders, those given with
/// `--allow-root` included, and applies at once, also to agent connections that are already
/// open. Each folder must be an absolute path without `..` that names an existing folder; it is
/// stored canonical, with symlinks resolved (`project.info` reports the result as
/// `allowed_roots`). UI credential only: agents get `forbidden`. Not undoable, not journaled,
/// never marks the project dirty.
#[command(
    name = "project.set_allowed_roots",
    summary = "Set the folders agents may use besides the open project's (the app's own setting)",
    engine,
    scope = "ui",
    undoable = false,
    journaled = false,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectSetAllowedRoots {
    /// Absolute paths of existing folders (at most 64); an empty list leaves only the project.
    pub roots: Vec<String>,
}

impl EngineCommand for ProjectSetAllowedRoots {
    type Context = SessionCall;
    type Output = ProjectSetAllowedRootsOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectSetAllowedRootsOutput, CmdError> {
        if self.roots.len() > MAX_ALLOWED_ROOTS {
            return Err(CmdError::invalid(
                "/roots",
                format!("give at most {MAX_ALLOWED_ROOTS} folders"),
            ));
        }
        let mut roots: Vec<PathBuf> = Vec::with_capacity(self.roots.len());
        for (i, raw) in self.roots.iter().enumerate() {
            let root = canonical_folder(&format!("/roots/{i}"), raw)?;
            if !roots.iter().any(|r| same_path(r, &root)) {
                roots.push(root);
            }
        }
        let out = roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect();
        s.core.set_approved_roots(roots);
        Ok(ProjectSetAllowedRootsOutput { roots: out })
    }
}

/// The folder `raw` names (the param at `pointer`), canonical: absolute, no `..`, existing, a
/// folder, symlinks resolved.
pub(crate) fn canonical_folder(pointer: &str, raw: &str) -> Result<PathBuf, CmdError> {
    if raw.trim().is_empty() || raw.len() > MAX_ROOT_LEN || raw.contains('\0') {
        return Err(CmdError::invalid(
            pointer,
            "must be the absolute path of a folder",
        ));
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(CmdError::invalid(pointer, "must be an absolute path"));
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(CmdError::invalid(pointer, "must not contain `..`"));
    }
    let real = std::fs::canonicalize(path).map_err(|e| {
        CmdError::invalid(
            pointer,
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("`{raw}` does not exist")
            } else {
                format!("`{raw}` cannot be read: {e}")
            },
        )
    })?;
    if !real.is_dir() {
        return Err(CmdError::invalid(
            pointer,
            format!("`{raw}` is not a folder"),
        ));
    }
    Ok(plain(real))
}

/// `path` without the `\\?\` prefix Windows' `canonicalize` adds, when the plain form names the
/// same folder (a drive path or a UNC share, shorter than the old `MAX_PATH`).
#[cfg(windows)]
fn plain(path: PathBuf) -> PathBuf {
    const MAX_PATH: usize = 260;
    let Some(text) = path.to_str() else {
        return path;
    };
    let simple = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        let drive = rest.as_bytes();
        if drive.len() >= 3 && drive[0].is_ascii_alphabetic() && drive[1] == b':' {
            rest.to_owned()
        } else {
            return path;
        }
    } else {
        return path;
    };
    if simple.len() < MAX_PATH {
        PathBuf::from(simple)
    } else {
        path
    }
}

#[cfg(not(windows))]
fn plain(path: PathBuf) -> PathBuf {
    path
}

/// Whether two canonical folders are the same (ASCII case-insensitively on Windows).
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    } else {
        a == b
    }
}

/// The folders a caller may name, for display (`project.info`'s `allowed_roots`): without
/// duplicates, in order.
pub(crate) fn distinct(roots: Vec<PathBuf>) -> Vec<String> {
    let mut out: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        if !out.iter().any(|r| same_path(r, &root)) {
            out.push(root);
        }
    }
    out.into_iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect()
}

// ----- the connections -------------------------------------------------------------------------

/// One agent connection open on the engine (`project.connections`, and the payload of
/// `project.connection_opened` and `project.connection_closed`).
#[wire]
pub struct AgentConnection {
    /// The connection's number on this engine (never reused).
    #[specta(type = specta_typescript::Number)]
    pub id: u64,
    /// Who it is: `agent:<label>` (`agent:mcp` for MCP clients, `agent:cli` for the command
    /// line) or `plugin:<id>`.
    pub actor: Actor,
    /// When it opened, in milliseconds since the Unix epoch.
    pub connected_at: WallTime,
    /// The last command or query it called (also one it was refused), if any.
    pub last_command: Option<String>,
    /// When it called it.
    pub last_command_at: Option<WallTime>,
}

/// The agent connections open on the engine. The UI credential and the engine's own internal
/// connection are not listed: they are not agents.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    open: Mutex<BTreeMap<u64, AgentConnection>>,
}

/// Whether connections of `ctx` are listed.
pub(crate) fn is_listed(ctx: &ActorCtx) -> bool {
    ctx.conn() != ConnId::INTERNAL && matches!(ctx.actor(), Actor::Agent(_) | Actor::Plugin(_))
}

impl Connections {
    /// `ctx` opened at `now`: listed and announced (if it is an agent).
    pub(crate) fn opened(&self, ctx: &ActorCtx, now: WallTime, events: &EventBus) {
        if !is_listed(ctx) {
            return;
        }
        let record = AgentConnection {
            id: ctx.conn().get(),
            actor: ctx.actor().clone(),
            connected_at: now,
            last_command: None,
            last_command_at: None,
        };
        self.lock().insert(record.id, record.clone());
        tracing::info!(conn = record.id, actor = %record.actor, "agent connected");
        events.publish(EngineEvent::Session(SessionEvent::ConnectionOpened(record)));
    }

    /// `ctx` closed: no longer listed, announced with its last state.
    pub(crate) fn closed(&self, ctx: &ActorCtx, events: &EventBus) {
        if !is_listed(ctx) {
            return;
        }
        let Some(record) = self.lock().remove(&ctx.conn().get()) else {
            return;
        };
        tracing::info!(conn = record.id, actor = %record.actor, "agent disconnected");
        events.publish(EngineEvent::Session(SessionEvent::ConnectionClosed(record)));
    }

    /// `ctx` called the registered entry `name` at `now`.
    pub(crate) fn called(&self, ctx: &ActorCtx, name: &str, now: WallTime) {
        if !is_listed(ctx) || name.len() > MAX_COMMAND_NAME {
            return;
        }
        if let Some(record) = self.lock().get_mut(&ctx.conn().get()) {
            name.clone_into(record.last_command.get_or_insert_with(String::new));
            record.last_command_at = Some(now);
        }
    }

    /// The connections open now, oldest first.
    pub(crate) fn list(&self) -> Vec<AgentConnection> {
        self.lock().values().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, AgentConnection>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// ----- project.connections ---------------------------------------------------------------------

/// Lists the agent connections open on this engine: who (`agent:mcp`, `agent:cli`), since
/// when, and the last command each called and when. The person's own editor is not listed.
/// `project.connection_opened` and `project.connection_closed` announce changes. Read-only.
#[wire]
pub struct ProjectConnections {}

/// Result of `project.connections`.
#[wire]
pub struct ProjectConnectionsOutput {
    /// The open agent connections, oldest first.
    pub connections: Vec<AgentConnection>,
}

impl Described for ProjectConnections {
    const SPEC: CommandSpec = CommandSpec {
        name: "project.connections",
        summary: "List the agent connections open on this engine, with their last command",
        kind: EntryKind::Engine,
        undoable: false,
        view: false,
        scope: Scope::Any,
        mcp: McpExposure::Catalog,
        annotations: Annotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        },
        secret_fields: &[],
        rust_type: "ProjectConnections",
    };
}

impl Registrable for ProjectConnections {
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError> {
        registry
            .add_engine_command::<ProjectConnections>()
            .map(drop)
    }
}

impl EngineCommand for ProjectConnections {
    type Context = SessionCall;
    type Output = ProjectConnectionsOutput;

    fn run(&self, s: &mut SessionCall) -> Result<ProjectConnectionsOutput, CmdError> {
        Ok(ProjectConnectionsOutput {
            connections: s.core.connections.list(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cutweave-agents-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn folders_must_be_absolute_existing_folders_without_dots() {
        let dir = temp("canon");
        let file = dir.join("a.mp4");
        std::fs::write(&file, b"x").unwrap();
        let text = |p: &Path| p.to_string_lossy().into_owned();
        let code = |raw: &str| canonical_folder("/roots/0", raw).unwrap_err();
        assert!(code("").to_string().contains("/roots/0"));
        assert!(code("relative/dir").to_string().contains("absolute"));
        assert!(
            code(&text(&dir.join("sub").join("..")))
                .to_string()
                .contains("..")
        );
        assert!(
            code(&text(&dir.join("missing")))
                .to_string()
                .contains("does not exist")
        );
        assert!(code(&text(&file)).to_string().contains("not a folder"));
        let ok = canonical_folder("/roots/0", &text(&dir)).unwrap();
        assert!(ok.is_absolute() && ok.is_dir());
        assert!(
            !ok.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            ok.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_resolved() {
        let dir = temp("link");
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let got = canonical_folder("/roots/0", &link.to_string_lossy()).unwrap();
        assert_eq!(got, std::fs::canonicalize(&real).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_agent_and_plugin_connections_are_listed_and_announced() {
        let bus = EventBus::new(8);
        let mut rx = bus.subscribe();
        let list = Connections::default();
        let ui = ActorCtx::ui(ConnId::new(5));
        let agent = ActorCtx::agent(ConnId::new(6), "mcp").unwrap();
        list.opened(&ui, WallTime::from_millis(1), &bus);
        list.opened(&ActorCtx::system(), WallTime::from_millis(1), &bus);
        list.opened(&agent, WallTime::from_millis(2), &bus);
        list.called(&agent, "project.info", WallTime::from_millis(3));
        list.called(&ui, "project.info", WallTime::from_millis(3));
        let open = list.list();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].actor.to_string(), "agent:mcp");
        assert_eq!(open[0].last_command.as_deref(), Some("project.info"));
        assert_eq!(open[0].last_command_at, Some(WallTime::from_millis(3)));
        list.closed(&ui, &bus);
        list.closed(&agent, &bus);
        assert_eq!(list.list(), Vec::new());
        let opened = rx.try_recv().unwrap();
        assert_eq!(opened.name(), "project.connection_opened");
        assert_eq!(opened.payload()["actor"], "agent:mcp");
        let closed = rx.try_recv().unwrap();
        assert_eq!(closed.name(), "project.connection_closed");
        assert_eq!(closed.payload()["last_command"], "project.info");
        assert!(rx.try_recv().is_err());
    }
}
