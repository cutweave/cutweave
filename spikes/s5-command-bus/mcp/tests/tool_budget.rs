// SPDX-License-Identifier: GPL-3.0-or-later
//! Acceptance test 2: `tools/list` size per profile against the arch §14 budgets (heuristic in
//! `s5_mcp::budget`: budgets are checked against the pessimistic bytes/3 estimate), plus the
//! catalog rules ADR-009 relies on: profiles nest, order is deterministic, annotations and input
//! schemas come verbatim from the registry, UI-only entries are never listed, and every
//! non-hidden entry is reachable through the generic tools.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Revision, engine, in_process};
use s5_bus::McpTier;
use s5_mcp::{
    Catalog, Profile,
    budget::{self, TABLE_HEADER},
    catalog::{self, MINIMAL_CORE, P1_GATE},
};
use serde_json::{Value, json};

fn names(c: &Catalog) -> Vec<String> {
    c.tools().iter().map(|t| t.name.to_string()).collect()
}

#[test]
fn tools_list_fits_the_budgets() {
    println!("{TABLE_HEADER}");
    for profile in Profile::ALL {
        let size = budget::measure(&Catalog::shared(profile));
        println!("{}", size.row());
        let b = profile.budget();
        assert!(
            size.tools <= b.max_tools,
            "{profile:?}: {} tools > {}",
            size.tools,
            b.max_tools
        );
        assert!(
            size.upper() <= b.max_tokens,
            "{profile:?}: ~{} tokens (bytes/3) > {}",
            size.upper(),
            b.max_tokens
        );
    }
    // The WBS Phase 1 gate applies to the default endpoint.
    let standard = budget::measure(&Catalog::shared(Profile::Standard));
    assert!(standard.tools <= P1_GATE.max_tools);
    assert!(standard.upper() <= P1_GATE.max_tokens);
}

#[test]
fn heuristic_is_documented_and_monotonic() {
    assert_eq!(budget::estimate_tokens(4000), 1000);
    assert_eq!(budget::upper_tokens(3000), 1000);
    assert_eq!(budget::estimate_tokens(1), 1);
    assert!(budget::upper_tokens(10_000) > budget::estimate_tokens(10_000));
}

#[test]
fn profiles_nest_and_are_deterministic() {
    let minimal = Catalog::shared(Profile::Minimal);
    let standard = Catalog::shared(Profile::Standard);
    let full = Catalog::shared(Profile::Full);
    for n in names(&minimal) {
        assert!(names(&standard).contains(&n), "minimal ⊂ standard: {n}");
    }
    assert_eq!(names(&standard), names(&full), "full lists no extra tools");
    assert_eq!(
        names(&minimal),
        [
            "edit_undo",
            "render_frame",
            "timeline_get",
            "cutweave_search_commands",
            "cutweave_describe_command",
            "cutweave_execute",
            "cutweave_query"
        ]
    );
    assert_eq!(
        json!(Catalog::shared(Profile::Standard).tools()),
        json!(standard.tools()),
        "byte-identical across builds (prompt-cache friendly)"
    );
    // outputSchema only on /mcp/full (budget); structuredContent is returned everywhere.
    assert!(standard.tools().iter().all(|t| t.output_schema.is_none()));
    assert!(
        full.tools()
            .iter()
            .filter(|t| !t.name.starts_with("cutweave_"))
            .all(|t| t.output_schema.is_some())
    );
}

#[test]
fn entry_tools_mirror_registry_metadata() {
    let standard = Catalog::shared(Profile::Standard);
    let registry = standard.registry();
    for entry in registry.iter() {
        let listed = standard
            .tools()
            .iter()
            .find(|t| t.name == entry.meta.mcp_tool_name());
        let should_list = entry.meta.mcp == McpTier::Core && !entry.meta.ui_only;
        assert_eq!(listed.is_some(), should_list, "{}", entry.meta.name);
        let Some(tool) = listed else { continue };
        // Parity: inputSchema is the RPC params schema, unchanged.
        assert_eq!(
            Value::Object((*tool.input_schema).clone()),
            entry.params_schema()
        );
        assert!(
            tool.description
                .as_deref()
                .unwrap()
                .starts_with(entry.meta.summary)
        );
        let a = tool.annotations.as_ref().unwrap();
        let m = entry.meta.annotations;
        assert_eq!(a.read_only_hint, Some(m.read_only), "{}", entry.meta.name);
        assert_eq!(
            a.destructive_hint,
            Some(m.destructive),
            "{}",
            entry.meta.name
        );
        assert_eq!(a.idempotent_hint, Some(m.idempotent), "{}", entry.meta.name);
        assert_eq!(a.open_world_hint, Some(false));
    }
    for name in MINIMAL_CORE {
        assert_eq!(registry.get(name).unwrap().meta.mcp, McpTier::Core);
    }
    // Generic tools: read-only except execute (which can run destructive commands).
    for tool in standard
        .tools()
        .iter()
        .filter(|t| t.name.starts_with("cutweave_"))
    {
        let a = tool.annotations.as_ref().unwrap();
        assert_eq!(a.read_only_hint, Some(tool.name != catalog::EXECUTE));
        assert_eq!(a.destructive_hint, Some(tool.name == catalog::EXECUTE));
    }
    // Image outputs advertise image content and hide `data` from the output schema.
    let frame = registry.get("render.frame").unwrap();
    assert!(catalog::has_image_output(frame));
    let schema = catalog::entry_output_schema(frame);
    assert!(
        schema["properties"]["result"]["properties"]
            .get("data")
            .is_none()
    );
    assert!(
        schema["properties"]["result"]["properties"]
            .get("mime")
            .is_some()
    );
}

/// Parity guarantee (arch §14): every non-hidden registry entry is reachable through MCP with
/// the same schema, in every profile, under both revisions.
#[tokio::test]
async fn every_entry_is_reachable_via_generic_tools() {
    for profile in Profile::ALL {
        for revision in Revision::ALL {
            let client = in_process(engine(), profile, revision).await;
            let all = client
                .ok(
                    "cutweave_search_commands",
                    json!({ "query": "", "limit": 50 }),
                )
                .await;
            let catalog = Catalog::shared(profile);
            let expected: Vec<&str> = catalog.all_reachable().map(|e| e.meta.name).collect();
            let found: Vec<&str> = all["matches"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["name"].as_str().unwrap())
                .collect();
            assert_eq!(found, expected, "{profile:?} {revision:?}");
            for entry in catalog.all_reachable() {
                let d = client
                    .ok(
                        "cutweave_describe_command",
                        json!({ "name": entry.meta.mcp_tool_name() }),
                    )
                    .await;
                assert_eq!(d["name"], entry.meta.name);
                assert_eq!(d["params_schema"], entry.params_schema());
                assert_eq!(d["output_schema"], entry.output_schema());
            }
            client.close().await;
        }
    }
}
