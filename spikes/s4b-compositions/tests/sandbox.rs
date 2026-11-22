// SPDX-License-Identifier: GPL-3.0-or-later
//! Sandbox containment (WBS 0G.01 acceptance): every adversarial script is contained with a
//! structured error, the loop is interrupted within the time limit, the allocation bomb fails with
//! a catchable error, forbidden globals are undefined/throw, and the engine survives.

use std::time::{Duration, Instant};

use s4b::adversarial::{self, Expect};
use s4b::diagnostics::{ErrorKind, codes};
use s4b::runtime::{BindOptions, Limits, load_source, sandbox_globals};

fn case(name: &str) {
    let cases = adversarial::cases();
    let c = cases.iter().find(|c| c.name == name).expect("case exists");
    let o = adversarial::run(c, Limits::default());
    assert!(
        o.passed,
        "{name}: expected {:?}, got {} — {}",
        c.expect, o.actual, o.detail
    );
    assert!(o.survived, "{name}: runtime unusable afterwards");
}

macro_rules! cases {
    ($($fn_name:ident => $case:literal),* $(,)?) => {
        $( #[test] fn $fn_name() { case($case); } )*
        #[test]
        fn every_case_has_a_test() {
            let named = [$($case),*];
            for c in adversarial::cases() {
                assert!(named.contains(&c.name), "add a test for `{}`", c.name);
            }
        }
    };
}

cases! {
    fetch => "fetch",
    require_static => "require-static",
    require_obfuscated => "require-obfuscated",
    import_static => "import-static",
    import_dynamic => "import-dynamic",
    import_dynamic_runtime => "import-dynamic-runtime",
    process => "process",
    timers => "timers",
    eval => "eval",
    eval_obfuscated => "eval-obfuscated",
    new_function => "new-function",
    function_ctor_via_prototype => "function-ctor-via-prototype",
    async_function_ctor => "async-function-ctor",
    generator_function_ctor => "generator-function-ctor",
    date_now => "date-now",
    new_date => "new-date",
    performance_now => "performance-now",
    math_random => "math-random",
    infinite_loop => "infinite-loop",
    infinite_loop_catch => "infinite-loop-catch",
    regex_backtracking => "regex-backtracking",
    native_builtin_overrun => "native-builtin-overrun",
    microtask_flood => "microtask-flood",
    async_render => "async-render",
    huge_alloc => "huge-alloc",
    huge_alloc_catchable => "huge-alloc-catchable",
    alloc_bomb => "alloc-bomb",
    stack_overflow => "stack-overflow",
    scene_bomb => "scene-bomb",
    atomics_wait => "atomics-wait",
    serializer_tamper => "serializer-tamper",
    host_read_getter_trap => "host-read-getter-trap",
    host_read_getter_throw => "host-read-getter-throw",
    lock_tamper => "lock-tamper",
    console_flood => "console-flood",
}

const LOOP_AT_5: &str = r#"
import { defineComposition, Rect } from "@cutweave/compose";
export default defineComposition({
  id: "adv.loop-at-5",
  durationInFrames: 10,
  render: ({ frame }) => {
    if (frame === 5) { while (true) {} }
    return <Rect w={frame + 1} h={1} />;
  },
});
"#;

#[test]
fn loop_is_interrupted_within_the_limit_and_runtime_keeps_rendering() {
    let limits = Limits {
        frame_time: Duration::from_millis(50),
        ..Limits::default()
    };
    let comp = load_source("loop.tsx", LOOP_AT_5, limits).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    assert_eq!(ev.eval(4.0).expect("frame 4").nodes[0].w, Some(5.0));
    let t = Instant::now();
    let e = ev.eval(5.0).expect_err("frame 5 loops");
    let elapsed = t.elapsed();
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert_eq!(e.code(), codes::TIMEOUT);
    assert!(
        elapsed >= Duration::from_millis(50) && elapsed < Duration::from_millis(500),
        "interrupted after {elapsed:?}"
    );
    // The same runtime and binding keep working.
    assert_eq!(ev.eval(6.0).expect("frame 6").nodes[0].w, Some(7.0));
    assert_eq!(ev.eval(4.0).expect("frame 4 again").nodes[0].w, Some(5.0));
}

#[test]
fn deterministic_fuel_budget_interrupts_independently_of_the_clock() {
    let limits = Limits {
        frame_time: Duration::from_secs(60),
        fuel_polls: Some(50),
        ..Limits::default()
    };
    let comp = load_source("loop.tsx", LOOP_AT_5, limits).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    ev.eval(4.0).expect("frame 4 fits the budget");
    let p0 = comp.polls();
    let e = ev.eval(5.0).expect_err("frame 5 loops");
    assert_eq!(e.kind, ErrorKind::Timeout);
    assert!(
        e.diagnostics[0].message.contains("CPU budget of 50"),
        "{e:?}"
    );
    assert_eq!(
        comp.polls() - p0,
        51,
        "interrupted on exactly the 51st poll"
    );
    ev.eval(6.0).expect("frame 6 after running out of fuel");
}

#[test]
fn stack_overflow_is_caught_on_a_1_mib_native_stack() {
    // The Windows main thread has 1 MiB; the default JS limit (512 KiB) must fire before the
    // native stack overflows (a 1 MiB JS limit crashed `s4b sandbox` on the main thread).
    let t = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let cases = adversarial::cases();
            let c = cases
                .iter()
                .find(|c| c.name == "stack-overflow")
                .expect("case");
            adversarial::run(c, Limits::default())
        })
        .expect("spawn");
    let o = t.join().expect("no native stack overflow");
    assert!(o.passed, "{o:?}");
}

#[test]
fn memory_is_reclaimed_after_out_of_memory() {
    let src = r#"
import { defineComposition, Rect } from "@cutweave/compose";
export default defineComposition({
  id: "adv.oom-at-2", durationInFrames: 5,
  render: ({ frame }) => {
    if (frame === 2) { const a: string[] = []; for (;;) a.push("y".repeat(1 << 20) + a.length); }
    return <Rect w={1} h={1} />;
  },
});
"#;
    let comp = load_source("oom.tsx", src, Limits::default()).expect("load");
    let ev = comp.bind("{}", &BindOptions::default()).expect("bind");
    ev.eval(1.0).expect("frame 1");
    let before = comp.memory_used();
    let e = ev.eval(2.0).expect_err("oom");
    assert_eq!(e.kind, ErrorKind::OutOfMemory);
    ev.eval(3.0).expect("frame 3 after OOM");
    let after = comp.memory_used();
    assert!(
        after < before + (4 << 20),
        "heap not reclaimed: {before} -> {after}"
    );
}

#[test]
fn forbidden_globals_are_absent() {
    let comp = load_source("g.tsx", s4b::LOWER_THIRD_TSX, Limits::default()).expect("load");
    let globals = sandbox_globals(&comp).expect("globals");
    for absent in [
        "eval",
        "fetch",
        "require",
        "process",
        "setTimeout",
        "setInterval",
        "queueMicrotask",
        "Atomics",
        "SharedArrayBuffer",
        "WeakRef",
        "FinalizationRegistry",
        "XMLHttpRequest",
        "WebSocket",
        "std",
        "os",
        "navigator",
        "window",
        "document",
    ] {
        assert!(
            !globals.iter().any(|g| g == absent),
            "`{absent}` is visible: {globals:?}"
        );
    }
    for present in [
        "Date",
        "performance",
        "Math",
        "JSON",
        "console",
        "Map",
        "Promise",
    ] {
        assert!(
            globals.iter().any(|g| g == present),
            "`{present}` missing: {globals:?}"
        );
    }
}

#[test]
fn math_random_is_seeded_per_item_and_frame() {
    let src = r#"
import { defineComposition, Rect } from "@cutweave/compose";
export default defineComposition({
  id: "adv.rng", durationInFrames: 10,
  render: () => <Rect w={1 + Math.random()} h={1 + Math.random()} />,
});
"#;
    let sample = |seed: &str, frame: f64| {
        // A fresh runtime per sample: values must not depend on runtime history.
        let comp = load_source("rng.tsx", src, Limits::default()).expect("load");
        let opts = BindOptions {
            item_seed: seed.into(),
            ..BindOptions::default()
        };
        let ev = comp.bind("{}", &opts).expect("bind");
        let n = &ev.eval(frame).expect("eval").nodes[0];
        (n.w, n.h)
    };
    assert_eq!(sample("clip-a", 3.0), sample("clip-a", 3.0));
    assert_ne!(sample("clip-a", 3.0), sample("clip-a", 4.0));
    assert_ne!(sample("clip-a", 3.0), sample("clip-b", 3.0));
    let (w, h) = sample("clip-a", 3.0);
    assert_ne!(w, h, "successive calls differ");

    // Order independence within one runtime: frame 3 after frames 0..9 equals frame 3 cold.
    let comp = load_source("rng.tsx", src, Limits::default()).expect("load");
    let opts = BindOptions {
        item_seed: "clip-a".into(),
        ..BindOptions::default()
    };
    let ev = comp.bind("{}", &opts).expect("bind");
    for f in 0..10 {
        ev.eval(f64::from(f)).expect("eval");
    }
    let n = &ev.eval(3.0).expect("eval").nodes[0];
    assert_eq!((n.w, n.h), sample("clip-a", 3.0));
}

#[test]
fn every_adversarial_case_is_contained_in_one_process() {
    // Run the whole suite back to back in one process (shared allocator, many runtimes).
    let outcomes: Vec<_> = adversarial::cases()
        .iter()
        .map(|c| adversarial::run(c, Limits::default()))
        .collect();
    let failed: Vec<_> = outcomes.iter().filter(|o| !o.passed).collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert!(outcomes.len() >= 30);
    assert!(
        adversarial::cases()
            .iter()
            .any(|c| matches!(c.expect, Expect::Compile(_)))
    );
}
