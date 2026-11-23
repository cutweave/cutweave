// SPDX-License-Identifier: GPL-3.0-or-later
//! WP-1I.04: the JSON log file and the panic hook's crash marker. One test only: the log
//! subscriber and the panic hook are process-global.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod engine_support;

use cutweave_engine::LogConfig;
use cutweave_engine::logging::{self, crash_marker_path};
use engine_support::TempDir;
use serde_json::Value;

#[test]
fn logs_are_json_lines_and_a_panic_leaves_a_crash_marker() {
    let t = TempDir::new("log");
    let guard = logging::init(&LogConfig::in_dir(t.path())).unwrap();
    let pid = std::process::id();
    assert_eq!(
        guard.path(),
        t.path().join(format!("engine-{pid}.log")).as_path()
    );
    {
        let span = tracing::info_span!("request", id = "req_1", method = "POST");
        let _entered = span.enter();
        tracing::info!(status = 200_u64, ok = true, "request done");
    }
    tracing::debug!("not written at INFO");

    let caught = std::thread::Builder::new()
        .name("doomed".into())
        .spawn(|| panic!("boom at the engine"))
        .unwrap()
        .join();
    assert!(caught.is_err());
    let marker = crash_marker_path(t.path(), pid);
    let crash: Value = serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    assert_eq!(crash["event"], "crash");
    assert_eq!(crash["pid"], pid);
    assert_eq!(crash["thread"], "doomed");
    assert_eq!(crash["message"], "boom at the engine");
    assert!(crash["location"].as_str().unwrap().contains("logging.rs"));

    let text = std::fs::read_to_string(guard.path()).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let done = lines
        .iter()
        .find(|l| l["msg"] == "request done")
        .expect("the event was not logged");
    assert_eq!(done["level"], "INFO");
    assert_eq!(done["fields"]["status"], 200);
    assert_eq!(done["fields"]["ok"], true);
    assert_eq!(done["spans"][0]["name"], "request");
    assert_eq!(done["spans"][0]["id"], "req_1");
    assert!(lines.iter().all(|l| l["msg"] != "not written at INFO"));
    let panic = lines
        .iter()
        .find(|l| l["target"] == "cutweave_engine::panic")
        .expect("the panic was not logged");
    assert_eq!(panic["fields"]["message"], "boom at the engine");

    logging::remove_crash_marker(t.path());
    assert!(!marker.exists());
    guard.finish();
}
