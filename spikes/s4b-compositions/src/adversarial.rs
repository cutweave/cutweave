// SPDX-License-Identifier: GPL-3.0-or-later
//! Adversarial compositions (WBS 0G.01): every one must be contained — rejected at compile time
//! with a structured diagnostic, or failing at runtime with a structured error, with the host
//! process and the runtime surviving. Shared by `tests/sandbox.rs` and `s4b sandbox`.

use std::time::Instant;

use serde::Serialize;

use crate::diagnostics::{ComposeError, ErrorKind, codes};
use crate::runtime::{BindOptions, Composition, Limits};
use crate::scene::FrameOutput;
use crate::transpile::{Compiled, SourceMapTable, TranspileTimings, transpile};

/// Expected containment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Rejected by the transpiler with this code.
    Compile(&'static str),
    /// Module evaluation fails with this kind.
    Load(ErrorKind),
    /// Frame evaluation fails with this kind and code.
    Frame(ErrorKind, &'static str),
    /// Frame succeeds and `check` passes.
    Ok,
}

/// One adversarial script.
pub struct Case {
    pub name: &'static str,
    pub what: &'static str,
    pub source: String,
    /// Skip the transpiler (and its policy checks) to test the runtime layer on its own.
    pub raw_js: bool,
    pub expect: Expect,
    pub check: Option<Check>,
}

/// Post-condition on a successful frame.
pub type Check = fn(&FrameOutput) -> Result<(), String>;

/// Wraps `attack` (statements run inside `render`) in a composition returning `ret` (JSX).
fn wrap(attack: &str, ret: &str) -> String {
    format!(
        "import {{ defineComposition, Rect, Text, Group }} from \"@cutweave/compose\";\n\
         export default defineComposition({{\n  id: \"adv.case\",\n  durationInFrames: 30,\n  fps: 30,\n  \
         render: ({{ frame }}) => {{\n    {attack}\n    return {ret};\n  }},\n}});\n"
    )
}

fn rect() -> &'static str {
    "<Rect w={10} h={10} />"
}

fn first_w(out: &FrameOutput) -> Option<f64> {
    out.nodes.first().and_then(|n| n.w)
}

/// The full adversarial suite.
#[must_use]
pub fn cases() -> Vec<Case> {
    use ErrorKind as K;
    let c = |name, what, source: String, expect| Case {
        name,
        what,
        source,
        raw_js: false,
        expect,
        check: None,
    };
    vec![
        // ---- I/O ---------------------------------------------------------------------------
        c("fetch", "network via fetch()", wrap("fetch(\"https://example.com\");", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("require-static", "require('fs')", wrap("require(\"fs\");", rect()),
          Expect::Compile(codes::IMPORT_NOT_ALLOWED)),
        c("require-obfuscated", "globalThis['req'+'uire']('fs')",
          wrap("(globalThis as any)[\"req\" + \"uire\"](\"fs\");", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("import-static", "import fs from 'fs'",
          format!("import fs from \"fs\";\n{}", wrap("fs;", rect())),
          Expect::Compile(codes::IMPORT_NOT_ALLOWED)),
        c("import-dynamic", "import('fs')", wrap("import(\"fs\");", rect()),
          Expect::Compile(codes::DYNAMIC_IMPORT)),
        Case {
            name: "import-dynamic-runtime",
            what: "await import('fs') with the transpiler bypassed (no module loader)",
            source: "import { defineComposition } from \"@cutweave/compose\";\n\
                     await import(\"fs\");\n\
                     export default defineComposition({ id: \"adv.raw\", durationInFrames: 1, render: () => [] });\n"
                .into(),
            raw_js: true,
            expect: Expect::Load(K::Runtime),
            check: None,
        },
        c("process", "process.env", wrap("(globalThis as any).process.env.HOME;", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("timers", "setTimeout / queueMicrotask",
          wrap("(globalThis as any).setTimeout(() => {}, 0);", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        // ---- Code generation ---------------------------------------------------------------
        c("eval", "eval('1+1')", wrap("eval(\"1+1\");", rect()), Expect::Compile(codes::CODE_GENERATION)),
        c("eval-obfuscated", "(0, globalThis['ev'+'al'])('1')",
          wrap("(0, (globalThis as any)[\"ev\" + \"al\"])(\"1\");", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("new-function", "new Function('return this')",
          wrap("new Function(\"return this\")();", rect()), Expect::Compile(codes::CODE_GENERATION)),
        c("function-ctor-via-prototype", "(function(){}).constructor('return globalThis')()",
          wrap("(function () {}).constructor(\"return globalThis\")();", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("async-function-ctor", "(async () => {}).constructor('...')",
          wrap("(async () => {}).constructor(\"return 1\");", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("generator-function-ctor", "(function* () {}).constructor('...')",
          wrap("(function* () {}).constructor(\"yield 1\");", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        // ---- Clock and RNG -----------------------------------------------------------------
        Case {
            check: Some(|o| match first_w(o) {
                Some(w) if (w - 101.0).abs() < 1e-9 => Ok(()),
                other => Err(format!("Date.now()+1 at frame 3 / 30 fps should be 101, got {other:?}")),
            }),
            ..c("date-now", "Date.now() is composition time (frame 3 @ 30 fps = 100 ms)",
                wrap("const t = Date.now();", "<Rect w={t + 1} h={1} />"), Expect::Ok)
        },
        c("new-date", "new Date() (wall clock)", wrap("new Date();", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        Case {
            check: Some(|o| match first_w(o) {
                Some(w) if (w - 101.0).abs() < 1e-9 => Ok(()),
                other => Err(format!("performance.now()+1 should be 101, got {other:?}")),
            }),
            ..c("performance-now", "performance.now() is composition time",
                wrap("const t = performance.now();", "<Rect w={t + 1} h={1} />"), Expect::Ok)
        },
        Case {
            check: Some(|o| match first_w(o) {
                Some(w) if w > 0.0 && w <= 1000.0 => Ok(()),
                other => Err(format!("seeded Math.random out of range: {other:?}")),
            }),
            ..c("math-random", "Math.random() is seeded (value checked for determinism in tests)",
                wrap("const r = Math.random();", "<Rect w={1 + r * 999} h={1} />"), Expect::Ok)
        },
        // ---- CPU ---------------------------------------------------------------------------
        c("infinite-loop", "while (true) {}", wrap("while (true) {}", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("infinite-loop-catch", "loop that catches the interrupt and loops again",
          wrap("for (;;) { try { while (true) {} } catch (e) { /* swallow */ } }", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("regex-backtracking", "catastrophic regex /(a+)+$/",
          wrap("/(a+)+$/.test(\"a\".repeat(40) + \"b\");", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("native-builtin-overrun",
          "default-comparator sort of 600k elements: native code the interrupt cannot preempt",
          wrap("const a = new Array(600000).fill(frame); a.sort();", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("microtask-flood", "endless await chain queued by render()",
          wrap("(async () => { for (;;) await 0; })();", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("async-render", "render() returns a Promise", wrap("", "Promise.resolve(1) as any"),
          Expect::Frame(K::Runtime, codes::ASYNC_RENDER)),
        // ---- Memory ------------------------------------------------------------------------
        c("huge-alloc", "new ArrayBuffer(1 GiB)", wrap("new ArrayBuffer(1 << 30);", rect()),
          Expect::Frame(K::OutOfMemory, codes::OUT_OF_MEMORY)),
        Case {
            check: Some(|o| {
                let t = o.nodes.first().and_then(|n| n.text.clone()).unwrap_or_default();
                if t.contains("out of memory") { Ok(()) } else { Err(format!("not caught in JS: {t:?}")) }
            }),
            ..c("huge-alloc-catchable", "1 GiB allocation inside try/catch is a catchable error",
                wrap("let msg = \"\"; try { new ArrayBuffer(1 << 30); } catch (e) { msg = String(e); }",
                     "<Text>{msg}</Text>"), Expect::Ok)
        },
        c("alloc-bomb", "unbounded growth (1 MiB strings)",
          wrap("const a: string[] = []; for (;;) a.push(\"x\".repeat(1 << 20) + a.length);", rect()),
          Expect::Frame(K::OutOfMemory, codes::OUT_OF_MEMORY)),
        c("stack-overflow", "unbounded recursion",
          wrap("const f = (n: number): number => f(n + 1) + 1; f(0);", rect()),
          Expect::Frame(K::StackOverflow, codes::STACK_OVERFLOW)),
        c("scene-bomb", "render() returns 20,000 plain-object nodes (limit 10,000)",
          wrap("const many: object[] = []; for (let i = 0; i < 20000; i++) many.push({ type: \"rect\", w: 1, h: 1 });",
               "many as any"),
          Expect::Frame(K::Scene, codes::SCENE_TOO_LARGE)),
        // ---- Blocking / realm --------------------------------------------------------------
        c("atomics-wait", "Atomics.wait (would block the engine thread)",
          wrap("(globalThis as any).Atomics.wait(new Int32Array(4), 0, 0);", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        Case {
            check: Some(|o| match first_w(o) {
                Some(w) if (w - 10.0).abs() < 1e-9 => Ok(()),
                other => Err(format!("scene altered: w = {other:?}")),
            }),
            ..c("serializer-tamper", "Object.prototype.toJSON (no effect: the host reads objects, not JSON)",
                wrap("(Object.prototype as any).toJSON = () => \"pwned\";", rect()), Expect::Ok)
        },
        c("host-read-getter-trap", "endless getter on Object.prototype hit by the host's scene read",
          wrap("Object.defineProperty(Object.prototype, \"blur\", { get() { for (;;) {} } });", rect()),
          Expect::Frame(K::Timeout, codes::TIMEOUT)),
        c("host-read-getter-throw", "throwing getter on Object.prototype hit by the host's scene read",
          wrap("Object.defineProperty(Object.prototype, \"src\", { get() { throw new Error(\"trap\"); } });", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        c("lock-tamper", "redefine Function.prototype.constructor",
          wrap("Object.defineProperty(Function.prototype, \"constructor\", { value: 1 });", rect()),
          Expect::Frame(K::Runtime, codes::EXCEPTION)),
        Case {
            check: Some(|o| {
                if o.logs.len() == 50 && o.dropped_logs == 9_950 { Ok(()) }
                else { Err(format!("logs {} dropped {}", o.logs.len(), o.dropped_logs)) }
            }),
            ..c("console-flood", "10,000 console.log lines are capped per frame (50 kept)",
                wrap("for (let i = 0; i < 10000; i++) console.log(\"spam\", i);", rect()), Expect::Ok)
        },
    ]
}

/// Result of running one case.
#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub name: &'static str,
    pub what: &'static str,
    pub passed: bool,
    /// What happened (error code/kind, or "ok").
    pub actual: String,
    pub detail: String,
    pub elapsed_ms: f64,
    /// The same runtime renders a normal frame afterwards.
    pub survived: bool,
}

fn describe(e: &ComposeError) -> (String, String) {
    let d = e.diagnostics.first();
    (
        format!("{:?} {}", e.kind, e.code()),
        d.map(crate::diagnostics::Diagnostic::render)
            .unwrap_or_default(),
    )
}

/// A frame of a well-behaved composition, used to prove the runtime still works after an attack.
fn sane_frame(comp: &Composition) -> bool {
    // A fresh binding of the same definition must still evaluate (or fail the same structured
    // way); the runtime itself must not be wedged. We probe with a trivial global script.
    crate::runtime::sandbox_globals(comp).is_ok()
}

/// Runs one case under `limits`.
#[must_use]
pub fn run(case: &Case, limits: Limits) -> Outcome {
    let t = Instant::now();
    let compiled = if case.raw_js {
        Ok(Compiled {
            file: "adversarial.js".into(),
            js: case.source.clone(),
            warnings: Vec::new(),
            map: SourceMapTable::default(),
            timings: TranspileTimings::default(),
        })
    } else {
        transpile("adversarial.tsx", &case.source)
    };
    let mut out = Outcome {
        name: case.name,
        what: case.what,
        passed: false,
        actual: String::new(),
        detail: String::new(),
        elapsed_ms: 0.0,
        survived: true,
    };
    let finish = |mut o: Outcome, passed: bool, (actual, detail): (String, String)| {
        o.passed = passed && o.survived;
        o.actual = actual;
        o.detail = detail;
        o.elapsed_ms = t.elapsed().as_secs_f64() * 1e3;
        o
    };
    let compiled = match compiled {
        Err(e) => {
            let ok = matches!(case.expect, Expect::Compile(code) if code == e.code());
            return finish(out, ok, describe(&e));
        }
        Ok(c) => c,
    };
    let comp = match Composition::load(&compiled, limits) {
        Err(e) => {
            let ok = matches!(case.expect, Expect::Load(kind) if kind == e.kind);
            return finish(out, ok, describe(&e));
        }
        Ok(c) => c,
    };
    let opts = BindOptions::default();
    let result = comp.bind("{}", &opts).and_then(|ev| ev.eval(3.0));
    out.survived = sane_frame(&comp);
    match result {
        Err(e) => {
            let ok = matches!(case.expect, Expect::Frame(kind, code) if kind == e.kind && code == e.code());
            finish(out, ok, describe(&e))
        }
        Ok(frame) => {
            let (ok, detail) = match (case.expect, case.check) {
                (Expect::Ok, Some(check)) => match check(&frame) {
                    Ok(()) => (true, "check passed".to_string()),
                    Err(m) => (false, m),
                },
                (Expect::Ok, None) => (true, String::new()),
                _ => (false, "unexpectedly succeeded".to_string()),
            };
            finish(out, ok, ("ok".to_string(), detail))
        }
    }
}
