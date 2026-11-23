// SPDX-License-Identifier: GPL-3.0-or-later
//! The [`Command`], [`Query`] and [`EngineCommand`] traits and the [`Registry`] (arch §5.1).
//!
//! Registration is explicit (arch §5.1, decided after Spike S5): each crate that defines
//! commands exposes `register(&mut Registry)`, and the engine's composition root calls them
//! all. Nothing depends on link-time collection, so it behaves the same on native and wasm32
//! and nothing is silently dropped by the linker.
//!
//! ```
//! # use cutweave_commands::{Registry, RegistryError};
//! pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
//!     // registry.register::<SplitItem>()?.register::<TrimItem>()?;
//!     Ok(())
//! }
//! let mut registry = Registry::new();
//! register(&mut registry).unwrap();
//! ```
//!
//! Registering checks each entry against the contract (name grammar and namespace, summary,
//! flag consistency, object schemas, secret fields) and rejects duplicate names, so a bad
//! entry fails at engine start-up (and in tests), never at call time.

use std::{
    any::{Any, TypeId, type_name},
    collections::BTreeMap,
};

use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use specta::{Types, datatype::DataType};

use crate::{
    ctx::{Access, QueryCtx},
    error::{CmdError, ResourceKind, decode_params, encode_output},
    meta::{CommandMeta, CommandSpec, EntryKind, McpExposure, Scope, check_name, check_summary},
    tx::Tx,
};

/// Placeholder that replaces `#[secret]` params in redacted copies.
pub const REDACTED: &str = "[redacted]";

/// The static metadata of a params type. Implemented by `#[command]` / `#[query]`, or by hand.
pub trait Described {
    /// The entry's metadata.
    const SPEC: CommandSpec;

    /// Whether successful calls are journaled for crash recovery (WP-1C.02); `true` by
    /// default. Only a non-undoable engine command whose effect is not engine state the
    /// journal replays may opt out (`job.cancel`: jobs are not persisted), with
    /// `#[command(engine, journaled = false, ...)]`. Read-only entries and `view.*` commands
    /// are never journaled regardless (see [`CommandMeta::is_journaled`]).
    const JOURNALED: bool = true;
}

/// A document command: mutates the project through a [`Tx`].
///
/// Write `impl Command for X` next to `#[command(...)] struct X`. `apply` must only change the
/// document through `tx`; returning `Err` rolls back everything it did.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is registered as a command but does not implement `cutweave_commands::Command`",
    label = "missing `impl Command for {Self}`",
    note = "add `impl Command for {Self} {{ type Output = ...; fn apply(&self, tx: &mut Tx) -> Result<Self::Output, CmdError> {{ ... }} }}`"
)]
pub trait Command: Described + DeserializeOwned + JsonSchema + specta::Type + 'static {
    /// The result (an object on the wire).
    type Output: Serialize + JsonSchema + specta::Type + 'static;

    /// Semantic checks before anything is applied (IDs exist, ranges, locks), returning
    /// `Invalid` / `NotFound` / `Locked`. Runs on every call, and alone for dry runs
    /// ([`Registry::validate_json`]). Defaults to no checks.
    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        let _ = ctx;
        Ok(())
    }

    /// Applies the command.
    fn apply(&self, tx: &mut Tx<'_>) -> Result<Self::Output, CmdError>;
}

/// A read-only query: it gets an immutable snapshot, so it cannot mutate by construction.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is registered as a query but does not implement `cutweave_commands::Query`",
    label = "missing `impl Query for {Self}`",
    note = "add `impl Query for {Self} {{ type Output = ...; fn run(&self, ctx: &QueryCtx) -> Result<Self::Output, CmdError> {{ ... }} }}`"
)]
pub trait Query: Described + DeserializeOwned + JsonSchema + specta::Type + 'static {
    /// The result (an object on the wire).
    type Output: Serialize + JsonSchema + specta::Type + 'static;

    /// Runs the query.
    fn run(&self, ctx: &QueryCtx<'_>) -> Result<Self::Output, CmdError>;
}

/// A command that runs against engine state instead of the document: history and transaction
/// control (`edit.undo`, `edit.begin_tx`, WP-1B.04) and UI view state (`view.*`, WP-1C.04).
///
/// The engine passes its state as `&mut dyn Any` ([`Registry::invoke_engine_json`]); the
/// registry hands it to the command as `Context`, so this crate does not need to know the
/// engine's types.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is registered as an engine command but does not implement `cutweave_commands::EngineCommand`",
    label = "missing `impl EngineCommand for {Self}`",
    note = "add `impl EngineCommand for {Self} {{ type Context = ...; type Output = ...; fn run(&self, ctx: &mut Self::Context) -> Result<Self::Output, CmdError> {{ ... }} }}`"
)]
pub trait EngineCommand:
    Described + DeserializeOwned + JsonSchema + specta::Type + 'static
{
    /// The engine state it runs against.
    type Context: Any;
    /// The result (an object on the wire).
    type Output: Serialize + JsonSchema + specta::Type + 'static;

    /// Runs the command.
    fn run(&self, ctx: &mut Self::Context) -> Result<Self::Output, CmdError>;
}

/// A params type that knows how to register itself (implemented by the macros), so a crate's
/// `register` reads `registry.register::<SplitItem>()?`.
pub trait Registrable {
    /// Adds this entry to `registry`.
    fn register_into(registry: &mut Registry) -> Result<(), RegistryError>;
}

/// A registration that breaks the contract. Each one is a programming error; the engine
/// refuses to start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// Two entries share a name.
    #[error("duplicate command name `{name}`")]
    Duplicate {
        /// The name.
        name: String,
    },
    /// The name is not `namespace.verb_object` in a known namespace.
    #[error("invalid command name `{name}`: {reason}")]
    InvalidName {
        /// The name.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// The summary is empty, too long or multi-line.
    #[error("`{name}`: summary {reason}")]
    InvalidSummary {
        /// The entry.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// Flags that contradict each other or the entry kind.
    #[error("`{name}`: {reason}")]
    InconsistentFlags {
        /// The entry.
        name: String,
        /// What is wrong.
        reason: String,
    },
    /// A params or output schema whose root is not an object (MCP requires objects).
    #[error(
        "`{name}`: the {which} schema root must be a JSON object (use a struct with named fields)"
    )]
    NotAnObject {
        /// The entry.
        name: String,
        /// `"input"` or `"output"`.
        which: &'static str,
    },
    /// A `#[secret]` field that is not a top-level param.
    #[error("`{name}`: secret field `{field}` is not a top-level param")]
    UnknownSecretField {
        /// The entry.
        name: String,
        /// The field.
        field: String,
    },
}

type ValidateFn = fn(&QueryCtx<'_>, Value) -> Result<(), CmdError>;

#[derive(Clone, Copy)]
enum Runner {
    Command(fn(&mut Tx<'_>, Value) -> Result<Value, CmdError>),
    Query(fn(&QueryCtx<'_>, Value) -> Result<Value, CmdError>),
    Engine {
        run: fn(&mut dyn Any, Value) -> Result<Value, CmdError>,
        context: TypeId,
    },
}

/// One registered command, query or engine command.
pub struct Entry {
    /// Metadata and schemas.
    pub meta: CommandMeta,
    runner: Runner,
    validate: ValidateFn,
    ts: fn(&mut Types) -> (DataType, DataType),
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("meta", &self.meta)
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// The wire name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.meta.name
    }

    /// Command, query or engine command.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        self.meta.kind
    }

    /// For an engine command, the [`TypeId`] of the `Context` it runs against (what
    /// [`Registry::invoke_engine_json`] must be given), so a host can route it; `None` for
    /// commands and queries.
    #[must_use]
    pub fn engine_context(&self) -> Option<TypeId> {
        match self.runner {
            Runner::Engine { context, .. } => Some(context),
            Runner::Command(_) | Runner::Query(_) => None,
        }
    }

    /// Registers the params and output TypeScript types in `types` and returns references to
    /// them (for the generated protocol package, WP-1N).
    pub fn ts_types(&self, types: &mut Types) -> (DataType, DataType) {
        (self.ts)(types)
    }

    /// A copy of `params` with every `#[secret]` param replaced by [`REDACTED`]: what may be
    /// journaled, logged or fingerprinted for deduplication (arch §14).
    #[must_use]
    pub fn redact(&self, params: &Value) -> Value {
        let mut out = params.clone();
        if let Value::Object(map) = &mut out {
            for field in self.meta.secret_fields {
                if let Some(v) = map.get_mut(*field)
                    && !v.is_null()
                {
                    *v = Value::String(REDACTED.to_owned());
                }
            }
        }
        out
    }
}

fn run_command<C: Command>(tx: &mut Tx<'_>, params: Value) -> Result<Value, CmdError> {
    let command: C = decode_params(params)?;
    command.validate(&tx.query_ctx())?;
    encode_output(&command.apply(tx)?)
}

fn validate_command<C: Command>(ctx: &QueryCtx<'_>, params: Value) -> Result<(), CmdError> {
    decode_params::<C>(params)?.validate(ctx)
}

fn run_query<Q: Query>(ctx: &QueryCtx<'_>, params: Value) -> Result<Value, CmdError> {
    encode_output(&decode_params::<Q>(params)?.run(ctx)?)
}

fn run_engine<E: EngineCommand>(ctx: &mut dyn Any, params: Value) -> Result<Value, CmdError> {
    let command: E = decode_params(params)?;
    let ctx = ctx.downcast_mut::<E::Context>().ok_or_else(|| {
        CmdError::internal(format!(
            "`{}` must run with a `{}` context",
            E::SPEC.name,
            type_name::<E::Context>()
        ))
    })?;
    encode_output(&command.run(ctx)?)
}

fn decode_only<T: DeserializeOwned>(_: &QueryCtx<'_>, params: Value) -> Result<(), CmdError> {
    decode_params::<T>(params).map(drop)
}

fn ts_pair<P: specta::Type, O: specta::Type>(types: &mut Types) -> (DataType, DataType) {
    (P::definition(types), O::definition(types))
}

fn schema_settings() -> SchemaSettings {
    // Self-contained schemas (no `$ref`/`$defs`) are the most portable form for MCP clients;
    // the catalog states `$schema` once at its root (WP-1N).
    SchemaSettings::draft2020_12().with(|s| {
        s.inline_subschemas = true;
        s.meta_schema = None;
    })
}

/// The JSON Schema of what the engine accepts for `T` (e.g. timecode strings for `TimeInput`).
#[must_use]
pub fn input_schema<T: JsonSchema>() -> Value {
    schema_settings()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value()
}

/// The JSON Schema of what the engine produces for `T`.
#[must_use]
pub fn output_schema<T: JsonSchema>() -> Value {
    schema_settings()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>()
        .to_value()
}

fn check_flags(spec: &CommandSpec) -> Result<(), String> {
    let namespace = spec.name.split_once('.').map_or("", |(ns, _)| ns);
    let a = spec.annotations;
    match spec.kind {
        EntryKind::Query => {
            if spec.undoable {
                return Err("a query cannot be undoable".into());
            }
            if !a.read_only || a.destructive || !a.idempotent {
                return Err("a query must be read-only, non-destructive and idempotent".into());
            }
            if spec.view {
                return Err("`view` marks commands, not queries".into());
            }
        }
        EntryKind::Command => {
            if a.read_only {
                return Err("a command is never read-only; make it a query".into());
            }
        }
        // An engine entry may only read engine state (`edit.history`): queries read the
        // document snapshot, which does not hold it.
        EntryKind::Engine => {
            if a.read_only && (spec.undoable || a.destructive || !a.idempotent || spec.view) {
                return Err(
                    "a read-only engine command must be non-undoable, non-destructive, \
                     idempotent and not a `view` command"
                        .into(),
                );
            }
        }
    }
    if spec.view {
        if spec.kind != EntryKind::Engine {
            return Err(
                "a `view` command runs on view state, so it must be an engine command".into(),
            );
        }
        if spec.undoable {
            return Err("a `view` command is never undoable".into());
        }
        if namespace != "view" {
            return Err("only `view.*` commands may be marked `view`".into());
        }
    } else if namespace == "view" && spec.kind != EntryKind::Query {
        return Err("`view.*` commands must be marked `view` (not undoable, not journaled)".into());
    }
    if spec.scope == Scope::UiOnly && spec.mcp == McpExposure::Core {
        return Err(
            "a UI-only command cannot be an MCP core tool: agent credentials can never call it"
                .into(),
        );
    }
    Ok(())
}

/// Only a non-undoable engine command may opt out of the journal (see
/// [`Described::JOURNALED`]): document commands change the document, which recovery replays.
fn check_journaled(spec: &CommandSpec, journaled: bool) -> Result<(), String> {
    if !journaled && (spec.kind != EntryKind::Engine || spec.undoable) {
        return Err(
            "only a non-undoable engine command can opt out of the journal (`journaled = false`)"
                .into(),
        );
    }
    Ok(())
}

fn is_object_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
}

/// Every command, query and engine command, by name.
#[derive(Debug, Default)]
pub struct Registry {
    entries: BTreeMap<&'static str, Entry>,
}

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Registers a macro-annotated params type (`#[command]` / `#[query]`).
    pub fn register<T: Registrable>(&mut self) -> Result<&mut Registry, RegistryError> {
        T::register_into(self)?;
        Ok(self)
    }

    /// Registers a document command.
    pub fn add_command<C: Command>(&mut self) -> Result<&mut Registry, RegistryError> {
        self.admit(
            C::SPEC,
            C::JOURNALED,
            EntryKind::Command,
            input_schema::<C>(),
            output_schema::<C::Output>(),
            Runner::Command(run_command::<C>),
            validate_command::<C>,
            ts_pair::<C, C::Output>,
        )
    }

    /// Registers a query.
    pub fn add_query<Q: Query>(&mut self) -> Result<&mut Registry, RegistryError> {
        self.admit(
            Q::SPEC,
            Q::JOURNALED,
            EntryKind::Query,
            input_schema::<Q>(),
            output_schema::<Q::Output>(),
            Runner::Query(run_query::<Q>),
            decode_only::<Q>,
            ts_pair::<Q, Q::Output>,
        )
    }

    /// Registers an engine command.
    pub fn add_engine_command<E: EngineCommand>(&mut self) -> Result<&mut Registry, RegistryError> {
        self.admit(
            E::SPEC,
            E::JOURNALED,
            EntryKind::Engine,
            input_schema::<E>(),
            output_schema::<E::Output>(),
            Runner::Engine {
                run: run_engine::<E>,
                context: TypeId::of::<E::Context>(),
            },
            decode_only::<E>,
            ts_pair::<E, E::Output>,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn admit(
        &mut self,
        spec: CommandSpec,
        journaled: bool,
        kind: EntryKind,
        mut input: Value,
        output: Value,
        runner: Runner,
        validate: ValidateFn,
        ts: fn(&mut Types) -> (DataType, DataType),
    ) -> Result<&mut Registry, RegistryError> {
        let name = spec.name.to_owned();
        check_name(spec.name).map_err(|reason| RegistryError::InvalidName {
            name: name.clone(),
            reason,
        })?;
        check_summary(spec.summary).map_err(|reason| RegistryError::InvalidSummary {
            name: name.clone(),
            reason,
        })?;
        if spec.kind != kind {
            return Err(RegistryError::InconsistentFlags {
                name,
                reason: format!(
                    "its spec says {:?} but it was registered as {kind:?}",
                    spec.kind
                ),
            });
        }
        check_flags(&spec)
            .and_then(|()| check_journaled(&spec, journaled))
            .map_err(|reason| RegistryError::InconsistentFlags {
                name: name.clone(),
                reason,
            })?;
        for (which, schema) in [("input", &input), ("output", &output)] {
            if !is_object_schema(schema) {
                return Err(RegistryError::NotAnObject { name, which });
            }
        }
        for field in spec.secret_fields {
            let property = input
                .get_mut("properties")
                .and_then(|p| p.get_mut(*field))
                .and_then(Value::as_object_mut)
                .ok_or_else(|| RegistryError::UnknownSecretField {
                    name: name.clone(),
                    field: (*field).to_owned(),
                })?;
            property.insert("writeOnly".into(), Value::Bool(true));
        }
        if self.entries.contains_key(spec.name) {
            return Err(RegistryError::Duplicate { name });
        }
        self.entries.insert(
            spec.name,
            Entry {
                meta: CommandMeta::new(spec, journaled, input, output),
                runner,
                validate,
                ts,
            },
        );
        Ok(self)
    }

    /// The entry with this wire name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }

    /// The entry with this MCP tool name (`clip_split`).
    #[must_use]
    pub fn get_by_tool_name(&self, tool: &str) -> Option<&Entry> {
        let (namespace, verb) = tool.split_once('_')?;
        self.get(&format!("{namespace}.{verb}"))
    }

    /// Every entry, sorted by name.
    pub fn list(&self) -> impl ExactSizeIterator<Item = &Entry> + '_ {
        self.entries.values()
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn entry(&self, name: &str) -> Result<&Entry, CmdError> {
        self.get(name)
            .ok_or_else(|| CmdError::not_found(ResourceKind::Command, name))
    }

    /// Runs a command (or a query, against the working document) by name with JSON params,
    /// inside `tx`. Checks the caller's scope, decodes the params (`Invalid` with the exact
    /// JSON Pointer on mismatch, unknown fields included), runs `validate` then `apply`, and
    /// encodes the output. A failing command is rolled back to where it started, so earlier
    /// commands in the same transaction are kept. Engine commands are refused (`Unsupported`):
    /// they are not part of document transactions.
    pub fn invoke_json(
        &self,
        name: &str,
        params: Value,
        tx: &mut Tx<'_>,
    ) -> Result<Value, CmdError> {
        let entry = self.entry(name)?;
        tx.access().check(&entry.meta)?;
        match entry.runner {
            Runner::Command(apply) => {
                let mark = tx.savepoint();
                let result = apply(tx, params);
                if result.is_err() {
                    tx.rollback_to(mark);
                }
                result
            }
            Runner::Query(run) => run(&tx.query_ctx(), params),
            Runner::Engine { .. } => Err(CmdError::unsupported(format!(
                "`{name}` acts on engine state and cannot run inside a document transaction"
            ))),
        }
    }

    /// Runs a query by name against a snapshot. Commands are refused (`Unsupported`): they
    /// need a transaction.
    pub fn query_json(
        &self,
        name: &str,
        params: Value,
        ctx: &QueryCtx<'_>,
    ) -> Result<Value, CmdError> {
        let entry = self.entry(name)?;
        ctx.access().check(&entry.meta)?;
        match entry.runner {
            Runner::Query(run) => run(ctx, params),
            Runner::Command(_) | Runner::Engine { .. } => Err(CmdError::unsupported(format!(
                "`{name}` is not a query; submit it as a command"
            ))),
        }
    }

    /// Checks a call without running it (dry run): scope, params, and for commands the
    /// `validate` hook against `ctx`.
    pub fn validate_json(
        &self,
        name: &str,
        params: Value,
        ctx: &QueryCtx<'_>,
    ) -> Result<(), CmdError> {
        let entry = self.entry(name)?;
        ctx.access().check(&entry.meta)?;
        (entry.validate)(ctx, params)
    }

    /// Runs an engine command by name. `ctx` must be the command's `Context` type (otherwise
    /// `Internal`).
    pub fn invoke_engine_json(
        &self,
        name: &str,
        params: Value,
        ctx: &mut dyn Any,
        access: Access,
    ) -> Result<Value, CmdError> {
        let entry = self.entry(name)?;
        access.check(&entry.meta)?;
        match entry.runner {
            Runner::Engine { run, context } => {
                if (*ctx).type_id() != context {
                    return Err(CmdError::internal(format!(
                        "`{name}` was given the wrong engine context"
                    )));
                }
                run(ctx, params)
            }
            Runner::Command(_) | Runner::Query(_) => Err(CmdError::unsupported(format!(
                "`{name}` is not an engine command"
            ))),
        }
    }
}
