// SPDX-License-Identifier: GPL-3.0-or-later
//! The whole engine suite at smoke scale (WP-2J.01): every probe once on a tiny project, with
//! a real engine process for the start and memory probes. It checks that the suite still runs
//! and what it writes, never how fast anything is.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use cutweave_bench::cli::finish;
use cutweave_bench::probes::{Scale, Workspace, run_all};
use cutweave_bench::result::{BenchResult, Budgets, Verdict, verdict};
use cutweave_bench::workspace_root;
use serde_json::Value;

fn scratch(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_suite_runs_end_to_end_at_smoke_scale() {
    let scale = Scale::smoke();
    let dir = scratch("suite-smoke");
    let workspace = Workspace::prepare(&scale, &dir, false).unwrap();
    let exe = Path::new(env!("CARGO_BIN_EXE_cutweave-bench"));
    let outcome = run_all(&workspace, &scale, exe).unwrap();

    let ids: Vec<&str> = outcome.metrics.iter().map(|m| m.id.as_str()).collect();
    for id in [
        "project_open_1h_ms",
        "timeline_get_p400_ms",
        "command_roundtrip_p95_ms",
        "command_rtt_in_process_p95_ms",
        "doc_patched_fanout_2_p95_ms",
        "placement_ripple_p5k_ms",
        "app_start_engine_ms",
        "idle_memory_no_media_mb",
    ] {
        assert!(ids.contains(&id), "no `{id}` in {ids:?}");
    }
    for m in &outcome.metrics {
        assert!(m.value.is_finite() && m.value > 0.0, "{m:?}");
        assert!(m.samples >= 1);
    }
    assert_eq!(outcome.rtt.in_process.samples, scale.commands);
    assert_eq!(outcome.rtt.websocket.samples, scale.commands);
    assert_eq!(
        outcome.rtt.websocket.histogram.iter().sum::<u32>() as usize,
        scale.commands
    );

    // The result file has the shape of bench/result.schema.json and the table names a verdict
    // for every metric.
    let root = workspace_root();
    let out = dir.join("result.json");
    let finished = finish(&root, &scale, &outcome, Some(&out)).unwrap();
    let text = std::fs::read_to_string(&finished.result_path).unwrap();
    let parsed: BenchResult = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.suite.as_deref(), Some("engine-smoke"));
    assert_eq!(parsed.metrics.len(), outcome.metrics.len());
    let raw: Value = serde_json::from_str(&text).unwrap();
    let schema: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("bench/result.schema.json")).unwrap())
            .unwrap();
    for key in schema["required"].as_array().unwrap() {
        assert!(raw.get(key.as_str().unwrap()).is_some(), "missing {key}");
    }
    let allowed = schema["properties"].as_object().unwrap();
    assert!(raw.as_object().unwrap().keys().all(|k| allowed.contains_key(k)));
    assert!(finished.histogram_path.is_file());
    assert!(finished.table.contains("command_roundtrip_p95_ms"));
    assert!(finished.table.contains("PASS") || finished.table.contains("FAIL"));

    // Every metric with a budget row is judged; the others are informational.
    let budgets = Budgets::load(&root).unwrap();
    let judged = parsed
        .metrics
        .iter()
        .filter(|m| verdict(&budgets, &parsed.machine, m) != Verdict::Info)
        .count();
    assert_eq!(judged, 4, "open, round trip, timeline.get and engine start");
}

#[test]
fn the_budget_rows_the_suite_reports_against_exist() {
    let budgets = Budgets::load(&workspace_root()).unwrap();
    for id in [
        "project_open_1h_ms",
        "command_roundtrip_p95_ms",
        "idle_memory_1h_1080p_mb",
        "timeline_get_p400_ms",
        "app_start_engine_ms",
    ] {
        assert!(budgets.get(id).is_some(), "no budget row `{id}`");
    }
}
