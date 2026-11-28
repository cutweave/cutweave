// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-2I.08 (ADR-0040, question 5): on P-400 a concise `timeline.get` at depth 2 is at most
//! half the bytes of the full one, and a concise read still names every item with its times.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use cutweave_bench::fixture::{FixtureSpec, generate};
use cutweave_commands::{Access, QueryCtx, Registry};
use serde_json::{Value, json};

fn registry() -> Registry {
    let mut registry = Registry::new();
    cutweave_ops::register(&mut registry).unwrap();
    registry
}

fn items(out: &Value) -> Vec<&Value> {
    out["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["items"].as_array().unwrap())
        .collect()
}

#[test]
fn a_concise_depth_2_read_of_p_400_is_at_most_half_the_bytes() {
    let fixture = generate(&FixtureSpec::p_400()).unwrap();
    let registry = registry();
    let ctx = QueryCtx::new(&fixture.project, Access::STANDARD);
    let read = |extra: Value| {
        let mut params = json!({"sequence": fixture.manifest.sequence, "depth": 2});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        registry.query_json("timeline.get", params, &ctx).unwrap()
    };
    let (default, full, concise) = (
        read(json!({})),
        read(json!({"detail": "full"})),
        read(json!({"detail": "concise"})),
    );
    assert_eq!(default, full, "`full` is the default");
    let bytes = |v: &Value| serde_json::to_vec(v).unwrap().len();
    let (full_bytes, concise_bytes) = (bytes(&full), bytes(&concise));
    println!("timeline.get depth 2 on P-400: full {full_bytes} bytes, concise {concise_bytes}");
    assert!(
        concise_bytes * 2 <= full_bytes,
        "concise is {concise_bytes} bytes, full {full_bytes}"
    );

    // The same items, with what an agent edits by: id, start and duration.
    let (all, short) = (items(&full), items(&concise));
    assert_eq!(all.len(), 400);
    assert_eq!(short.len(), 400);
    for (a, b) in all.iter().zip(&short) {
        for key in ["id", "start", "duration"] {
            assert_eq!(a[key], b[key], "{key}");
        }
    }
    for key in ["id", "name", "duration", "track_count", "item_count"] {
        assert_eq!(full[key], concise[key], "{key}");
    }
}
