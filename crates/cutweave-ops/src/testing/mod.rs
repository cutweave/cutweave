// SPDX-License-Identifier: GPL-3.0-or-later
//! The property-test harness interface (WP-1B.07; arch §5.2, §20), behind the `testing`
//! feature.
//!
//! - [`arb_command_for`] / [`arb_command_from`]: a registry-driven generator of
//!   `(name, params)` for **every registered non-`view` entry** (commands, queries and engine
//!   commands): IDs drawn from the document (plus unknown ones), times on the owning
//!   sequence's frame grid in every accepted input form (some off the grid, which edit
//!   commands snap), and a share of semantically invalid and malformed params.
//! - [`replay_events`] / [`JsonMirror`]: `doc.patched` events replayed onto a JSON mirror (the
//!   UI docStore contract).
//! - [`check_invariants`] / [`check_linked_edit`]: the model invariants of arch §4.3 and the
//!   linked-group rules of the WP-1B.06 edit semantics.
//! - [`job_manager`] / [`analyzer`]: the engine context of the `job.*` commands (seeded job IDs
//!   the strategies can name, with jobs already in several states) and the analysis jobs a
//!   harness's `media.import` queues (WP-1D.07).
//! - [`NoProxies`]: the proxy service of the proxy commands in a harness (WP-2F.01).
//! - [`export_ctx`]: the engine context of `export.start` (WP-1E.03), with a starter that
//!   queues nothing.
//!
//! The harness that drives them through the document actor and checks the properties of arch
//! §20 (undo/redo identity, failures change nothing, serialize round trip, invariants, event
//! replay) is `crates/cutweave-ops/tests/registry_props.rs`.
//!
//! # Adding a command
//!
//! Registering a command without a strategy fails the harness with `missing property strategy
//! for <name>` ([`missing_strategies`]). Add an arm to the table in `testing/strategies.rs`:
//! a weight and a params strategy built from the document's pools, mixing valid params with
//! invalid ones (`mix`). If the command runs against a new engine context, teach the harness
//! to run it too.

use std::sync::LazyLock;

use cutweave_commands::{Entry, Registry};
use cutweave_model::Project;
use proptest::{prelude::*, strategy::Union};
use serde_json::Value;

mod invariants;
mod mirror;
mod strategies;

pub use invariants::{check_invariants, check_linked_edit};
pub use mirror::{JsonMirror, ReplayError, replay_events};
pub use strategies::{
    AGENT_LABEL, BROKEN_IMPORT_PATH, CHECKPOINT_NAMES, EXPORT_FILE_NAME, EXPORT_JOB_ID,
    IMPORT_PATHS, JOB_ID_SEED, NoProxies, analyzer, export_ctx, fake_probe, job_manager,
    preset_ctx,
};

use strategies::{Pools, args_for};

/// What the engine's composition root registers and the harness drives: this crate's
/// commands, queries and `view.*` engine commands, plus the `edit.*` engine commands of
/// `cutweave-commands` (transactions, undo/redo, history, checkpoints).
///
/// # Panics
///
/// If registration fails (a contract violation in a command definition).
#[must_use]
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    if let Err(e) = crate::register(&mut registry) {
        panic!("cutweave-ops does not register: {e}");
    }
    if let Err(e) = cutweave_commands::edit::register(&mut registry) {
        panic!("edit.* does not register: {e}");
    }
    registry
}

static REGISTRY: LazyLock<Registry> = LazyLock::new(registry);

/// The entries the harness must cover: every registered entry except `view.*` (UI state, not
/// the document).
pub fn covered(registry: &Registry) -> impl Iterator<Item = &Entry> + '_ {
    registry.list().filter(|e| !e.meta.view)
}

/// `(name, params)` for any non-`view` entry of [`registry()`], with params drawn from `doc`.
///
/// # Panics
///
/// `missing property strategy for <name>` if a registered entry has no strategy.
pub fn arb_command_for(doc: &Project) -> BoxedStrategy<(String, Value)> {
    arb_command_from(&REGISTRY, doc)
}

/// [`arb_command_for`] over the entries of `registry`.
///
/// # Panics
///
/// `missing property strategy for <name>` if an entry of `registry` (other than `view.*`) has
/// no strategy.
pub fn arb_command_from(registry: &Registry, doc: &Project) -> BoxedStrategy<(String, Value)> {
    let pools = Pools::of(doc);
    let arms: Vec<(u32, BoxedStrategy<(String, Value)>)> = covered(registry)
        .map(|entry| {
            let name = entry.name();
            let Some((weight, params)) = args_for(name, &pools) else {
                panic!("missing property strategy for {name}");
            };
            let arm = params.prop_map(move |p| (name.to_owned(), p)).boxed();
            (weight, arm)
        })
        .collect();
    Union::new_weighted(arms).boxed()
}

/// [`arb_command_from`] restricted to the entries called `names`: for a focused property test
/// of a few commands (WP-2A.02 drives the track and marker commands this way).
///
/// # Panics
///
/// If a name is not registered or has no strategy.
pub fn arb_command_among(
    registry: &Registry,
    doc: &Project,
    names: &[&str],
) -> BoxedStrategy<(String, Value)> {
    let pools = Pools::of(doc);
    let arms: Vec<(u32, BoxedStrategy<(String, Value)>)> = names
        .iter()
        .map(|name| {
            let Some(entry) = registry.get(name) else {
                panic!("{name} is not registered");
            };
            let name = entry.name();
            let Some((weight, params)) = args_for(name, &pools) else {
                panic!("missing property strategy for {name}");
            };
            let arm = params.prop_map(move |p| (name.to_owned(), p)).boxed();
            (weight, arm)
        })
        .collect();
    Union::new_weighted(arms).boxed()
}

/// The entries of `registry` (other than `view.*`) that have no strategy.
#[must_use]
pub fn missing_strategies(registry: &Registry) -> Vec<&'static str> {
    let pools = Pools::default();
    covered(registry)
        .map(Entry::name)
        .filter(|name| args_for(name, &pools).is_none())
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use cutweave_model::testing::arb_project;
    use proptest::{strategy::ValueTree, test_runner::TestRunner};

    use super::*;

    #[test]
    fn every_entry_has_a_strategy() {
        assert_eq!(missing_strategies(&registry()), Vec::<&str>::new());
    }

    #[test]
    fn strategies_generate_objects_for_any_document() {
        let mut runner = TestRunner::deterministic();
        for _ in 0..32 {
            let doc = arb_project().new_tree(&mut runner).unwrap().current();
            let commands = arb_command_for(&doc);
            for _ in 0..16 {
                let (name, params) = commands.new_tree(&mut runner).unwrap().current();
                assert!(REGISTRY.get(&name).is_some(), "{name}");
                assert!(params.is_object(), "{name}: {params}");
            }
        }
    }
}
