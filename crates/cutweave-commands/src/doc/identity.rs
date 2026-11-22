// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is calling (arch §5.2, §6, ADR-0016): the [`Actor`] every transaction is tagged with, the
//! [`ConnId`] that owns open transactions, and [`ActorCtx`], which bundles both with the
//! credential's [`Access`].
//!
//! The engine derives an [`ActorCtx`] from the authenticated credential (UI credential →
//! [`ActorCtx::ui`], agent token → [`ActorCtx::agent`], plugin → [`ActorCtx::plugin`], internal
//! jobs → [`ActorCtx::system`]). It has no `Deserialize` impl and its fields are private, so a
//! client can never assert its own actor or access.

use std::{borrow::Cow, fmt, str::FromStr};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::ctx::Access;

/// Longest accepted agent label (`agent:<label>`), in characters.
pub const MAX_AGENT_LABEL_CHARS: usize = 64;

/// Longest accepted plugin id (`plugin:<id>`), in characters.
pub const MAX_PLUGIN_ID_CHARS: usize = 128;

/// Who made a change: the tag on every transaction, history entry and `doc.patched` event.
///
/// On the wire it is a string: `user`, `agent:<label>`, `plugin:<id>` or `system` (arch §5.2).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, specta::Type)]
#[specta(type = String)]
pub enum Actor {
    /// The person at the editor (the UI credential).
    User,
    /// An agent (MCP or CLI token), by the token's label.
    Agent(String),
    /// A plugin, by its id.
    Plugin(String),
    /// The engine itself (internal jobs, recovery).
    System,
}

/// An actor string that is not `user`, `system`, `agent:<label>` or `plugin:<id>`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid actor `{input}`: {reason}")]
pub struct ActorError {
    /// The rejected text.
    pub input: String,
    /// What is wrong.
    pub reason: &'static str,
}

fn check_agent_label(label: &str) -> Result<(), &'static str> {
    let chars = label.chars().count();
    if chars == 0 || chars > MAX_AGENT_LABEL_CHARS {
        return Err("an agent label must be 1..=64 characters");
    }
    if label.chars().any(char::is_control) {
        return Err("an agent label cannot contain control characters");
    }
    if label.trim() != label {
        return Err("an agent label cannot start or end with whitespace");
    }
    Ok(())
}

fn check_plugin_id(id: &str) -> Result<(), &'static str> {
    if id.is_empty() || id.chars().count() > MAX_PLUGIN_ID_CHARS {
        return Err("a plugin id must be 1..=128 characters");
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
    {
        return Err("a plugin id may only contain ASCII letters, digits and `._-@`");
    }
    Ok(())
}

impl Actor {
    /// `agent:<label>`, with the label of the agent's token.
    pub fn agent(label: impl Into<String>) -> Result<Actor, ActorError> {
        let label = label.into();
        match check_agent_label(&label) {
            Ok(()) => Ok(Actor::Agent(label)),
            Err(reason) => Err(ActorError {
                input: format!("agent:{label}"),
                reason,
            }),
        }
    }

    /// `plugin:<id>`.
    pub fn plugin(id: impl Into<String>) -> Result<Actor, ActorError> {
        let id = id.into();
        match check_plugin_id(&id) {
            Ok(()) => Ok(Actor::Plugin(id)),
            Err(reason) => Err(ActorError {
                input: format!("plugin:{id}"),
                reason,
            }),
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::User => f.write_str("user"),
            Actor::System => f.write_str("system"),
            Actor::Agent(label) => write!(f, "agent:{label}"),
            Actor::Plugin(id) => write!(f, "plugin:{id}"),
        }
    }
}

impl FromStr for Actor {
    type Err = ActorError;

    fn from_str(s: &str) -> Result<Actor, ActorError> {
        match s {
            "user" => Ok(Actor::User),
            "system" => Ok(Actor::System),
            _ => {
                if let Some(label) = s.strip_prefix("agent:") {
                    Actor::agent(label)
                } else if let Some(id) = s.strip_prefix("plugin:") {
                    Actor::plugin(id)
                } else {
                    Err(ActorError {
                        input: s.to_owned(),
                        reason: "expected `user`, `system`, `agent:<label>` or `plugin:<id>`",
                    })
                }
            }
        }
    }
}

impl Serialize for Actor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Actor, D::Error> {
        let s = Cow::<str>::deserialize(d)?;
        s.parse().map_err(de::Error::custom)
    }
}

impl JsonSchema for Actor {
    fn schema_name() -> Cow<'static, str> {
        "Actor".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "Who made a change: `user`, `agent:<label>`, `plugin:<id>` or `system`.",
            "pattern": "^(user|system|agent:.+|plugin:[A-Za-z0-9._@-]+)$"
        })
    }
}

/// A client connection (WebSocket, MCP session, stdio proxy), assigned by the API layer.
///
/// Open transactions are owned by the connection that began them (ADR-0017): only it may add
/// to, commit or cancel them, and closing it rolls them back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnId(u64);

impl ConnId {
    /// Requests that come from inside the engine (jobs, recovery), not from a connection.
    pub const INTERNAL: ConnId = ConnId(0);

    /// The connection with this number (assigned by the API layer; `0` is
    /// [`INTERNAL`](Self::INTERNAL)).
    #[must_use]
    pub const fn new(raw: u64) -> ConnId {
        ConnId(raw)
    }

    /// The raw number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ConnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "conn:{}", self.0)
    }
}

/// The authenticated caller of a request: its connection, its [`Actor`] and its [`Access`],
/// all derived by the engine from the credential (ADR-0016).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActorCtx {
    conn: ConnId,
    actor: Actor,
    access: Access,
}

impl ActorCtx {
    /// The UI credential on `conn`: actor `user`, every scope.
    #[must_use]
    pub fn ui(conn: ConnId) -> ActorCtx {
        ActorCtx {
            conn,
            actor: Actor::User,
            access: Access::UI,
        }
    }

    /// An agent token labelled `label` on `conn`: actor `agent:<label>`, no UI-only scopes.
    pub fn agent(conn: ConnId, label: impl Into<String>) -> Result<ActorCtx, ActorError> {
        Ok(ActorCtx {
            conn,
            actor: Actor::agent(label)?,
            access: Access::STANDARD,
        })
    }

    /// A plugin credential: actor `plugin:<id>`, no UI-only scopes.
    pub fn plugin(conn: ConnId, id: impl Into<String>) -> Result<ActorCtx, ActorError> {
        Ok(ActorCtx {
            conn,
            actor: Actor::plugin(id)?,
            access: Access::STANDARD,
        })
    }

    /// The engine itself (internal jobs): actor `system` on [`ConnId::INTERNAL`], no UI-only
    /// scopes.
    #[must_use]
    pub fn system() -> ActorCtx {
        ActorCtx {
            conn: ConnId::INTERNAL,
            actor: Actor::System,
            access: Access::STANDARD,
        }
    }

    /// The connection.
    #[must_use]
    pub const fn conn(&self) -> ConnId {
        self.conn
    }

    /// The actor.
    #[must_use]
    pub const fn actor(&self) -> &Actor {
        &self.actor
    }

    /// What the credential may invoke.
    #[must_use]
    pub const fn access(&self) -> Access {
        self.access
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn actors_round_trip_as_strings() {
        for (actor, text) in [
            (Actor::User, "user"),
            (Actor::System, "system"),
            (Actor::agent("claude").unwrap(), "agent:claude"),
            (
                Actor::agent("Claude Desktop").unwrap(),
                "agent:Claude Desktop",
            ),
            (
                Actor::plugin("cutweave.fx.blur@1").unwrap(),
                "plugin:cutweave.fx.blur@1",
            ),
        ] {
            assert_eq!(actor.to_string(), text);
            assert_eq!(text.parse::<Actor>().unwrap(), actor);
            let json = serde_json::to_value(&actor).unwrap();
            assert_eq!(json, serde_json::Value::String(text.into()));
            assert_eq!(serde_json::from_value::<Actor>(json).unwrap(), actor);
        }
    }

    #[test]
    fn malformed_actors_are_rejected() {
        for bad in [
            "",
            "User",
            "agent:",
            "agent: padded",
            "agent:line\nbreak",
            "plugin:",
            "plugin:has space",
            "robot:x",
        ] {
            assert!(bad.parse::<Actor>().is_err(), "{bad:?}");
        }
        assert!(Actor::agent("x".repeat(MAX_AGENT_LABEL_CHARS)).is_ok());
        assert!(Actor::agent("x".repeat(MAX_AGENT_LABEL_CHARS + 1)).is_err());
        assert!(serde_json::from_str::<Actor>("\"agent:\"").is_err());
    }

    #[test]
    fn contexts_bind_actor_and_access() {
        let ui = ActorCtx::ui(ConnId::new(3));
        assert_eq!(ui.actor(), &Actor::User);
        assert_eq!(ui.access(), Access::UI);
        assert_eq!(ui.conn(), ConnId::new(3));
        let agent = ActorCtx::agent(ConnId::new(4), "claude").unwrap();
        assert_eq!(agent.actor().to_string(), "agent:claude");
        assert_eq!(agent.access(), Access::STANDARD);
        let system = ActorCtx::system();
        assert_eq!(system.conn(), ConnId::INTERNAL);
        assert_eq!(system.access(), Access::STANDARD);
        assert_eq!(ConnId::new(9).to_string(), "conn:9");
    }
}
