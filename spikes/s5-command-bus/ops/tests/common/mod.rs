//! Shared test helpers.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_pass_by_value
)]

use std::sync::Arc;

use s5_bus::{
    Caller, Engine, EngineConfig,
    engine::{ManualClock, Request},
    time::FLICKS_PER_SECOND,
};
use serde_json::{Value, json};

/// Deterministic engine over the full registry (forces s5-ops to link).
pub fn engine() -> Engine {
    let _ = s5_ops::registry();
    Engine::new(EngineConfig::deterministic(42))
}

/// Deterministic engine with a manual clock.
pub fn engine_with_clock() -> (Engine, ManualClock) {
    let _ = s5_ops::registry();
    let clock = ManualClock::default();
    let engine = Engine::new(EngineConfig::deterministic(42).with_clock(Arc::new(clock.clone())));
    (engine, clock)
}

pub fn agent() -> Caller {
    Caller::agent("test")
}

pub fn secs(s: i64) -> i64 {
    s * FLICKS_PER_SECOND
}

/// Calls through the JSON surface (what MCP/RPC/CLI use) and returns the full reply.
pub fn call_json(engine: &mut Engine, caller: &Caller, name: &str, params: Value) -> Value {
    engine.invoke_json(caller, json!({ "name": name, "params": params }))
}

/// Calls and unwraps the result, panicking with the error otherwise.
pub fn ok(engine: &mut Engine, caller: &Caller, name: &str, params: Value) -> Value {
    match engine.invoke(caller, Request::new(name, params)) {
        Ok(r) => r.result,
        Err(e) => panic!("{name} failed: {e} ({})", e.to_wire()),
    }
}

/// Calls and returns the wire error, panicking on success.
pub fn err(engine: &mut Engine, caller: &Caller, name: &str, params: Value) -> Value {
    match engine.invoke(caller, Request::new(name, params)) {
        Ok(r) => panic!("{name} unexpectedly succeeded: {}", r.result),
        Err(e) => e.to_wire(),
    }
}

/// Imports a 10 s video and inserts it at 0; returns (media, item).
pub fn setup_clip(engine: &mut Engine, caller: &Caller) -> (String, String) {
    let media = ok(
        engine,
        caller,
        "media.import",
        json!({ "path": "/footage/a.mp4" }),
    )["media"]
        .as_str()
        .unwrap()
        .to_owned();
    let item = ok(
        engine,
        caller,
        "timeline.insert",
        json!({ "media": media, "at": 0 }),
    )["item"]
        .as_str()
        .unwrap()
        .to_owned();
    (media, item)
}
