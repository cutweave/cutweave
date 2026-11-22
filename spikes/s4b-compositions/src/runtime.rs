// SPDX-License-Identifier: GPL-3.0-or-later
//! The rquickjs (QuickJS-ng) host and sandbox.
//!
//! One [`Composition`] = one QuickJS runtime + context (not `Send`; Phase 1 would keep a runtime
//! per worker thread). The sandbox is built from what is *absent* plus a little hardening:
//!
//! * **No I/O**: QuickJS core has no `fetch`/`require`/`process`/`fs`/timers (those live in
//!   quickjs-libc, which is not linked); no module loader is installed, so `import()` of anything
//!   that is not pre-declared fails; the transpiler rejects foreign imports up front.
//! * **No clock**: the `Date` and `Performance` intrinsics are not added; `host.js` installs
//!   `Date.now()`/`performance.now()` returning the *composition* time of the frame.
//! * **Seeded RNG**: `Math.random` is mulberry32 reseeded per (item seed, frame).
//! * **No code generation from strings**: `eval` deleted, all Function constructors throw.
//! * **No blocking / GC-observable primitives**: `Atomics`, `SharedArrayBuffer`, `WeakRef`,
//!   `FinalizationRegistry` removed (Atomics.wait would also be refused: `can_block` is false).
//! * **Limits**: `JS_SetMemoryLimit` (catchable `out of memory`), `JS_SetMaxStackSize`, and an
//!   interrupt handler enforcing a wall-clock deadline per frame (uncatchable `interrupted`).

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::context::intrinsic::{
    Eval, Json, MapSet, Promise, RegExp, RegExpCompiler, TypedArrays,
};
use rquickjs::{Atom, Context, Ctx, Function, Module, Object, Persistent, Runtime, Value};
use serde::{Deserialize, Serialize};

use crate::diagnostics::{ComposeError, Diagnostic, ErrorKind, Phase, codes};
use crate::scene::{FrameOutput, RawNode};
use crate::transpile::{COMPOSE_MODULE, Compiled, JSX_RUNTIME_MODULE, SourceMapTable};

const COMPOSE_JS: &str = include_str!("../runtime/compose.js");
const JSX_RUNTIME_JS: &str = include_str!("../runtime/jsx-runtime.js");
const HOST_JS: &str = include_str!("../runtime/host.js");
const HOST_MODULE: &str = "cutweave:host";

/// Sandbox limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// QuickJS heap limit for the whole runtime.
    pub memory_bytes: usize,
    /// JS stack limit. QuickJS measures it on the *native* stack of the calling thread, so the
    /// thread must have this much plus host headroom: the Windows main thread has only 1 MiB
    /// (finding: a 1 MiB limit there overflows the native stack before QuickJS's check fires).
    pub stack_bytes: usize,
    /// Wall-clock budget for one `render(frame)` call (WBS default: 50 ms).
    pub frame_time: Duration,
    /// Budget for module evaluation (top-level code) and `bind`.
    pub load_time: Duration,
    /// Maximum scene nodes per frame.
    pub max_nodes: usize,
    /// Maximum scene JSON size per frame.
    pub max_scene_bytes: usize,
    /// `console.*` lines kept per frame.
    pub max_logs: u32,
    /// Heap growth that triggers QuickJS's cycle collector. Per-frame garbage is acyclic and
    /// freed by refcounting, so a high threshold mostly skips useless GC passes.
    pub gc_threshold_bytes: usize,
    /// Optional **deterministic** CPU budget per call, in interrupt-handler polls (QuickJS polls
    /// every 10,000 interrupt-counter ticks: calls and loop back-edges). Unlike the wall-clock
    /// deadline it does not depend on machine speed or load, so a frame that passes once passes
    /// everywhere. `None` = wall clock only (the WBS acceptance setting).
    pub fuel_polls: Option<u64>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 64 << 20,
            stack_bytes: 512 << 10,
            frame_time: Duration::from_millis(50),
            load_time: Duration::from_millis(500),
            max_nodes: 10_000,
            max_scene_bytes: 16 << 20,
            max_logs: 50,
            gc_threshold_bytes: std::env::var("S4B_GC_THRESHOLD")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(256 << 10),
            fuel_polls: None,
        }
    }
}

/// `describe()` result: metadata + props JSON Schema (what MCP `composition.create` returns).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompositionInfo {
    pub id: String,
    pub title: Option<String>,
    pub duration_in_frames: u32,
    pub fps: Option<f64>,
    pub props_schema: serde_json::Value,
}

/// Cold-start costs, microseconds.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct LoadTimings {
    /// Runtime + context creation.
    pub runtime_us: f64,
    /// `@cutweave/compose` + host module (hardening) evaluation.
    pub prelude_us: f64,
    /// Composition module compile + evaluation + `describe()`.
    pub module_us: f64,
}

/// Per-item binding parameters.
#[derive(Clone, Debug)]
pub struct BindOptions {
    pub width: u32,
    pub height: u32,
    /// Overrides the composition's fps (else its own, else 30).
    pub fps: Option<f64>,
    /// Seed for `Math.random` (the timeline item id in the product).
    pub item_seed: String,
}

impl Default for BindOptions {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: None,
            item_seed: "item-0".into(),
        }
    }
}

/// Interrupt-handler state shared with the runtime.
#[derive(Default)]
struct Guard {
    deadline: Cell<Option<Instant>>,
    fired: Cell<bool>,
    /// Set when the fuel budget (not the wall clock) interrupted the call.
    out_of_fuel: Cell<bool>,
    /// Interrupt-handler invocations (QuickJS polls every ~10k "ticks": a deterministic fuel
    /// measure, unlike wall time).
    polls: Cell<u64>,
    /// Poll count at which the current call runs out of fuel.
    fuel_end: Cell<Option<u64>>,
}

/// A loaded composition (one sandboxed QuickJS runtime).
pub struct Composition {
    // Field order = drop order: persistent handles before the context, context before runtime.
    definition: Persistent<Value<'static>>,
    bind_fn: Persistent<Function<'static>>,
    ctx: Context,
    rt: Runtime,
    guard: Rc<Guard>,
    file: String,
    map: SourceMapTable,
    limits: Limits,
    info: CompositionInfo,
    timings: LoadTimings,
}

/// Evaluates frames of a composition bound to one set of props.
pub struct FrameEvaluator<'c> {
    comp: &'c Composition,
    func: Persistent<Function<'static>>,
    fps: f64,
}

fn us(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e6
}

/// Properties of a thrown value.
#[derive(Default)]
struct Thrown {
    name: Option<String>,
    message: String,
    stack: Option<String>,
    code: Option<String>,
    path: Option<String>,
}

fn thrown(ctx: &Ctx<'_>) -> Thrown {
    let v = ctx.catch();
    if let Some(obj) = v.as_object() {
        let get = |k: &str| obj.get::<_, Option<String>>(k).ok().flatten();
        Thrown {
            name: get("name"),
            message: get("message").unwrap_or_default(),
            stack: get("stack"),
            code: get("code"),
            path: get("path"),
        }
    } else if let Some(s) = v.as_string() {
        Thrown {
            message: s.to_string().unwrap_or_default(),
            ..Thrown::default()
        }
    } else {
        Thrown {
            message: format!("non-Error value thrown ({})", v.type_name()),
            ..Thrown::default()
        }
    }
}

impl Composition {
    /// Creates the sandbox, evaluates the prelude and the composition module, reads its metadata.
    pub fn load(compiled: &Compiled, limits: Limits) -> Result<Self, ComposeError> {
        let t = Instant::now();
        let engine = |e: rquickjs::Error| ComposeError::engine(format!("QuickJS: {e}"));
        let rt = Runtime::new().map_err(engine)?;
        rt.set_memory_limit(limits.memory_bytes);
        rt.set_max_stack_size(limits.stack_bytes);
        rt.set_gc_threshold(limits.gc_threshold_bytes);
        let guard = Rc::new(Guard::default());
        {
            let g = Rc::clone(&guard);
            rt.set_interrupt_handler(Some(Box::new(move || {
                let polls = g.polls.get() + 1;
                g.polls.set(polls);
                if g.fuel_end.get().is_some_and(|end| polls > end) {
                    g.out_of_fuel.set(true);
                    g.fired.set(true);
                    return true;
                }
                match g.deadline.get() {
                    Some(d) if Instant::now() >= d => {
                        g.fired.set(true);
                        true
                    }
                    _ => false,
                }
            })));
        }
        // No Date, Performance, WeakRef, Proxy or DOMException intrinsics.
        let ctx = Context::custom::<(
            Eval,
            Json,
            MapSet,
            RegExpCompiler,
            RegExp,
            TypedArrays,
            Promise,
        )>(&rt)
        .map_err(engine)?;
        let mut timings = LoadTimings {
            runtime_us: us(t),
            ..LoadTimings::default()
        };

        let mut partial = PartialHost {
            ctx: &ctx,
            guard: &guard,
            file: &compiled.file,
            map: &compiled.map,
            fuel: limits.fuel_polls,
        };
        let t = Instant::now();
        let (bind_fn, describe_fn) = partial.guarded(limits.load_time, |ctx| {
            Module::declare(ctx.clone(), COMPOSE_MODULE, COMPOSE_JS)?;
            Module::declare(ctx.clone(), JSX_RUNTIME_MODULE, JSX_RUNTIME_JS)?;
            let host = Module::declare(ctx.clone(), HOST_MODULE, HOST_JS)?;
            let (host, done) = host.eval()?;
            done.finish::<()>()?;
            let bind: Function = host.get("bind")?;
            let describe: Function = host.get("describe")?;
            Ok((Persistent::save(ctx, bind), Persistent::save(ctx, describe)))
        })?;
        timings.prelude_us = us(t);

        let t = Instant::now();
        let (definition, info_json) = partial.guarded(limits.load_time, |ctx| {
            let m = Module::declare(ctx.clone(), compiled.file.as_str(), compiled.js.as_str())?;
            let (m, done) = m.eval()?;
            done.finish::<()>()?;
            let def: Value = m.get("default")?;
            let describe = describe_fn.clone().restore(ctx)?;
            let info: String = describe.call((def.clone(),))?;
            Ok((Persistent::save(ctx, def), info))
        })?;
        drop(describe_fn);
        let info: CompositionInfo = serde_json::from_str(&info_json)
            .map_err(|e| ComposeError::engine(format!("describe(): {e}")))?;
        timings.module_us = us(t);
        partial.reset();

        Ok(Self {
            definition,
            bind_fn,
            ctx,
            rt,
            guard,
            file: compiled.file.clone(),
            map: compiled.map.clone(),
            limits,
            info,
            timings,
        })
    }

    /// Metadata and props JSON Schema.
    #[must_use]
    pub fn info(&self) -> &CompositionInfo {
        &self.info
    }

    /// Cold-start timings.
    #[must_use]
    pub fn timings(&self) -> LoadTimings {
        self.timings
    }

    /// Current QuickJS heap usage in bytes.
    #[must_use]
    pub fn memory_used(&self) -> i64 {
        self.rt.memory_usage().memory_used_size
    }

    /// Interrupt-handler polls so far (deterministic fuel counter).
    #[must_use]
    pub fn polls(&self) -> u64 {
        self.guard.polls.get()
    }

    /// Validates `props_json` against the schema (applying defaults) and returns an evaluator.
    pub fn bind(
        &self,
        props_json: &str,
        opts: &BindOptions,
    ) -> Result<FrameEvaluator<'_>, ComposeError> {
        let fps = opts.fps.or(self.info.fps).unwrap_or(30.0);
        let max_nodes = u32::try_from(self.limits.max_nodes).unwrap_or(u32::MAX);
        let host = self.host();
        let func = host.guarded(self.limits.load_time, |ctx| {
            let bind = self.bind_fn.clone().restore(ctx)?;
            let def = self.definition.clone().restore(ctx)?;
            let options = serde_json::json!({
                "width": opts.width,
                "height": opts.height,
                "fps": fps,
                "seed": opts.item_seed,
                "maxNodes": max_nodes,
                "maxLogs": self.limits.max_logs,
            })
            .to_string();
            let f: Function = bind.call((def, props_json, options))?;
            Ok(Persistent::save(ctx, f))
        })?;
        Ok(FrameEvaluator {
            comp: self,
            func,
            fps,
        })
    }

    fn host(&self) -> PartialHost<'_> {
        PartialHost {
            ctx: &self.ctx,
            guard: &self.guard,
            file: &self.file,
            map: &self.map,
            fuel: self.limits.fuel_polls,
        }
    }
}

impl FrameEvaluator<'_> {
    /// Frames per second the composition is evaluated at.
    #[must_use]
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Calls `renderFrame(frame, as_json)` under the frame deadline and converts the result.
    fn call<R>(
        &self,
        frame: f64,
        as_json: bool,
        convert: impl for<'js> FnOnce(&Ctx<'js>, Value<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, ComposeError> {
        let comp = self.comp;
        let host = comp.host();
        let r = host.guarded(comp.limits.frame_time, |ctx| {
            let f = self.func.clone().restore(ctx)?;
            let v: Value = f.call((frame, as_json))?;
            let r = convert(ctx, v)?;
            // Microtasks queued by render() cannot change the returned scene; drain them inside
            // the same deadline so they cannot pile up across frames. An endless chain is cut by
            // the interrupt (the interrupted job rejects, which ends the chain).
            while ctx.execute_pending_job() {
                if comp.guard.fired.get() {
                    break;
                }
            }
            Ok(r)
        })?;
        if comp.guard.out_of_fuel.get() {
            return Err(out_of_fuel(comp.limits.fuel_polls.unwrap_or_default()));
        }
        if comp.guard.fired.get() {
            return Err(timeout(comp.limits.frame_time, "frame"));
        }
        Ok(r)
    }

    /// Runs `render(frame)` and reads the returned node objects directly from the QuickJS heap
    /// (no JSON round trip). This is the fast path.
    pub fn eval(&self, frame: f64) -> Result<FrameOutput, ComposeError> {
        let out = self.call(frame, false, |ctx, v| {
            let keys = Keys::new(ctx)?;
            let obj = v
                .into_object()
                .ok_or_else(|| rquickjs::Error::new_from_js("value", "object"))?;
            read_output(&obj, &keys)
        })?;
        out.map_err(|msg| {
            ComposeError::new(
                ErrorKind::Scene,
                Diagnostic::error(
                    codes::SCENE_INVALID,
                    Phase::Scene,
                    format!("invalid scene: {msg}"),
                ),
            )
        })
    }

    /// Runs `render(frame)` and returns the scene as JSON produced in the sandbox (canonical
    /// bytes: what the determinism hashes cover).
    pub fn eval_json(&self, frame: f64) -> Result<String, ComposeError> {
        let comp = self.comp;
        let json = self.call(frame, true, |_, v| {
            v.into_string()
                .ok_or_else(|| rquickjs::Error::new_from_js("value", "string"))?
                .to_string()
        })?;
        if json.len() > comp.limits.max_scene_bytes {
            return Err(ComposeError::new(
                ErrorKind::Scene,
                Diagnostic::error(
                    codes::SCENE_TOO_LARGE,
                    Phase::Scene,
                    format!(
                        "scene JSON is {} bytes (limit {})",
                        json.len(),
                        comp.limits.max_scene_bytes
                    ),
                ),
            ));
        }
        Ok(json)
    }

    /// Runs `render(frame)` via JSON (`eval_json` + serde), for comparison with [`Self::eval`].
    pub fn eval_via_json(&self, frame: f64) -> Result<FrameOutput, ComposeError> {
        parse_frame(&self.eval_json(frame)?)
    }
}

/// Property-name atoms, created once per frame.
struct Keys<'js> {
    kind: Atom<'js>,
    id: Atom<'js>,
    x: Atom<'js>,
    y: Atom<'js>,
    w: Atom<'js>,
    h: Atom<'js>,
    radius: Atom<'js>,
    fill: Atom<'js>,
    opacity: Atom<'js>,
    rotation: Atom<'js>,
    scale: Atom<'js>,
    anchor: Atom<'js>,
    blend: Atom<'js>,
    blur: Atom<'js>,
    text: Atom<'js>,
    font: Atom<'js>,
    size: Atom<'js>,
    src: Atom<'js>,
    frame: Atom<'js>,
    children: Atom<'js>,
    nodes: Atom<'js>,
    logs: Atom<'js>,
    dropped: Atom<'js>,
    level: Atom<'js>,
    message: Atom<'js>,
}

impl<'js> Keys<'js> {
    fn new(ctx: &Ctx<'js>) -> rquickjs::Result<Self> {
        let a = |s: &str| Atom::from_str(ctx.clone(), s);
        Ok(Self {
            kind: a("type")?,
            id: a("id")?,
            x: a("x")?,
            y: a("y")?,
            w: a("w")?,
            h: a("h")?,
            radius: a("radius")?,
            fill: a("fill")?,
            opacity: a("opacity")?,
            rotation: a("rotation")?,
            scale: a("scale")?,
            anchor: a("anchor")?,
            blend: a("blend")?,
            blur: a("blur")?,
            text: a("text")?,
            font: a("font")?,
            size: a("size")?,
            src: a("src")?,
            frame: a("frame")?,
            children: a("children")?,
            nodes: a("nodes")?,
            logs: a("logs")?,
            dropped: a("droppedLogs")?,
            level: a("level")?,
            message: a("message")?,
        })
    }
}

/// Nesting limit for the direct reader (serde_json's default recursion limit is 128).
const MAX_DEPTH: usize = 64;

type Read<T> = rquickjs::Result<Result<T, String>>;

/// `undefined` → None; numbers → Some; anything else (incl. null) → Some(NaN), reported by
/// lowering with the node path.
fn num(v: &Value<'_>) -> Option<f64> {
    if v.is_undefined() {
        None
    } else {
        Some(v.as_number().unwrap_or(f64::NAN))
    }
}

fn string(v: &Value<'_>) -> rquickjs::Result<Option<String>> {
    if v.is_undefined() || v.is_null() {
        return Ok(None);
    }
    match v.as_string() {
        Some(s) => s.to_string().map(Some),
        // Wrong type: keep a marker the lowering rejects ("fill must be a hex colour ...").
        None => Ok(Some(format!("<{}>", v.type_name()))),
    }
}

fn pair(v: &Value<'_>) -> rquickjs::Result<Option<[f64; 2]>> {
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(match v.as_array() {
        Some(a) if a.len() == 2 => {
            let get = |i| a.get::<Value>(i).map(|v| v.as_number().unwrap_or(f64::NAN));
            [get(0)?, get(1)?]
        }
        _ => [f64::NAN, f64::NAN],
    }))
}

fn read_node<'js>(o: &Object<'js>, k: &Keys<'js>, depth: usize) -> Read<RawNode> {
    let g = |a: &Atom<'js>| o.get::<_, Value>(a.clone());
    let kind = match g(&k.kind)?.as_string() {
        Some(s) => s.to_string()?,
        None => return Ok(Err("node without a string `type`".into())),
    };
    let mut children = Vec::new();
    let c = g(&k.children)?;
    if let Some(arr) = c.as_array() {
        if depth >= MAX_DEPTH {
            return Ok(Err(format!("scene nesting deeper than {MAX_DEPTH}")));
        }
        children.reserve(arr.len());
        for v in arr.iter::<Value>() {
            let Some(child) = v?.into_object() else {
                return Ok(Err("group children must be node objects".into()));
            };
            match read_node(&child, k, depth + 1)? {
                Ok(n) => children.push(n),
                Err(e) => return Ok(Err(e)),
            }
        }
    }
    Ok(Ok(RawNode {
        kind,
        id: string(&g(&k.id)?)?,
        x: num(&g(&k.x)?),
        y: num(&g(&k.y)?),
        w: num(&g(&k.w)?),
        h: num(&g(&k.h)?),
        radius: num(&g(&k.radius)?),
        fill: string(&g(&k.fill)?)?,
        opacity: num(&g(&k.opacity)?),
        rotation: num(&g(&k.rotation)?),
        scale: pair(&g(&k.scale)?)?,
        anchor: pair(&g(&k.anchor)?)?,
        blend: string(&g(&k.blend)?)?,
        blur: num(&g(&k.blur)?),
        text: string(&g(&k.text)?)?,
        font: string(&g(&k.font)?)?,
        size: num(&g(&k.size)?),
        src: string(&g(&k.src)?)?,
        frame: num(&g(&k.frame)?),
        children,
    }))
}

fn read_output<'js>(o: &Object<'js>, k: &Keys<'js>) -> Read<FrameOutput> {
    let mut out = FrameOutput::default();
    let nodes: Value = o.get(k.nodes.clone())?;
    let Some(arr) = nodes.as_array() else {
        return Ok(Err("`nodes` is not an array".into()));
    };
    out.nodes.reserve(arr.len());
    for v in arr.iter::<Value>() {
        let Some(obj) = v?.into_object() else {
            return Ok(Err("scene nodes must be objects".into()));
        };
        match read_node(&obj, k, 0)? {
            Ok(n) => out.nodes.push(n),
            Err(e) => return Ok(Err(e)),
        }
    }
    let logs: Value = o.get(k.logs.clone())?;
    if let Some(arr) = logs.as_array() {
        for v in arr.iter::<Object>() {
            let v = v?;
            out.logs.push(crate::scene::LogEntry {
                level: v.get(k.level.clone())?,
                message: v.get(k.message.clone())?,
            });
        }
    }
    out.dropped_logs = o.get::<_, Option<u32>>(k.dropped.clone())?.unwrap_or(0);
    Ok(Ok(out))
}

impl FrameEvaluator<'_> {
    /// Purity check (what `composition.validate` would run): evaluates `frames` forward, then in
    /// reverse, and requires byte-identical scene JSON. Catches module-level state carried across
    /// frames, which would make scrubbing, caching and parallel export disagree.
    pub fn check_pure(&self, frames: &[f64]) -> Result<(), ComposeError> {
        let forward: Vec<String> = frames
            .iter()
            .map(|&f| self.eval_json(f))
            .collect::<Result<_, _>>()?;
        for (i, &f) in frames.iter().enumerate().rev() {
            if self.eval_json(f)? != forward[i] {
                return Err(ComposeError::new(
                    ErrorKind::Runtime,
                    Diagnostic::error(
                        codes::IMPURE,
                        Phase::Runtime,
                        format!(
                            "render() returned a different scene for frame {f} on re-evaluation"
                        ),
                    )
                    .with_help(
                        "render() must be a pure function of { frame, fps, props, size }: \
                         do not keep state in module variables or globals",
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Parses scene JSON produced by [`FrameEvaluator::eval_json`].
pub fn parse_frame(json: &str) -> Result<FrameOutput, ComposeError> {
    serde_json::from_str(json).map_err(|e| {
        ComposeError::new(
            ErrorKind::Scene,
            Diagnostic::error(
                codes::SCENE_INVALID,
                Phase::Scene,
                format!("invalid scene: {e}"),
            ),
        )
    })
}

fn timeout(limit: Duration, what: &str) -> ComposeError {
    ComposeError::new(
        ErrorKind::Timeout,
        Diagnostic::error(
            codes::TIMEOUT,
            Phase::Runtime,
            format!(
                "{what} evaluation exceeded the {} ms time limit and was interrupted",
                limit.as_millis()
            ),
        )
        .with_help("render() must be a fast pure function of frame/props; avoid unbounded loops"),
    )
}

fn out_of_fuel(polls: u64) -> ComposeError {
    ComposeError::new(
        ErrorKind::Timeout,
        Diagnostic::error(
            codes::TIMEOUT,
            Phase::Runtime,
            format!(
                "evaluation exceeded the CPU budget of {polls} interrupt polls and was interrupted"
            ),
        )
        .with_help("render() must be a fast pure function of frame/props; avoid unbounded loops"),
    )
}

/// The pieces needed to run guarded calls and classify errors (usable before `Composition`
/// exists, during `load`).
struct PartialHost<'a> {
    ctx: &'a Context,
    guard: &'a Guard,
    file: &'a str,
    map: &'a SourceMapTable,
    fuel: Option<u64>,
}

impl PartialHost<'_> {
    fn reset(&mut self) {
        self.guard.deadline.set(None);
        self.guard.fuel_end.set(None);
        self.guard.fired.set(false);
        self.guard.out_of_fuel.set(false);
    }

    /// Runs `f` with a deadline (and fuel budget); JS exceptions become structured errors.
    fn guarded<R>(
        &self,
        limit: Duration,
        f: impl for<'js> FnOnce(&Ctx<'js>) -> rquickjs::Result<R>,
    ) -> Result<R, ComposeError> {
        self.guard.fired.set(false);
        self.guard.out_of_fuel.set(false);
        self.guard
            .fuel_end
            .set(self.fuel.map(|f| self.guard.polls.get() + f));
        let start = Instant::now();
        self.guard.deadline.set(Some(start + limit));
        let mut r = self
            .ctx
            .with(|ctx| f(&ctx).map_err(|e| self.classify(&ctx, e, limit)));
        self.guard.deadline.set(None);
        self.guard.fuel_end.set(None);
        // The interrupt is only polled in bytecode and the regex engine: a long native builtin
        // (e.g. a big default-comparator sort) can finish past the deadline. Report it anyway.
        if r.is_ok() && start.elapsed() > limit {
            let mut e = timeout(limit, "frame");
            e.diagnostics[0].message.push_str(
                " (it finished late, inside a native builtin the interrupt cannot preempt)",
            );
            r = Err(e);
        }
        if r.is_err() {
            // Reclaim cycles left by an aborted frame (refcounting frees the rest).
            self.ctx.with(|ctx| ctx.run_gc());
        }
        r
    }

    fn classify(&self, ctx: &Ctx<'_>, err: rquickjs::Error, limit: Duration) -> ComposeError {
        match err {
            rquickjs::Error::Exception => {
                let t = thrown(ctx);
                if self.guard.out_of_fuel.get() {
                    return out_of_fuel(self.fuel.unwrap_or_default());
                }
                if self.guard.fired.get() {
                    return timeout(limit, "frame");
                }
                self.exception(t)
            }
            rquickjs::Error::Allocation => ComposeError::new(
                ErrorKind::OutOfMemory,
                Diagnostic::error(codes::OUT_OF_MEMORY, Phase::Runtime, "out of memory"),
            ),
            other => ComposeError::engine(format!("QuickJS: {other}")),
        }
    }

    /// Maps a JS stack (`at fn (file:line:col)`) to source positions; returns the frames and
    /// the first frame inside the composition file.
    fn map_stack(&self, stack: &str) -> (Vec<String>, Option<(u32, u32)>) {
        let mut frames = Vec::new();
        let mut first = None;
        for line in stack.lines() {
            let l = line.trim();
            let Some(rest) = l.strip_prefix("at ") else {
                continue;
            };
            let (func, loc) = match rest.rfind(" (") {
                Some(i) if rest.ends_with(')') => (&rest[..i], &rest[i + 2..rest.len() - 1]),
                _ => ("<anonymous>", rest),
            };
            let mut parts = loc.rsplitn(3, ':');
            let col = parts.next().and_then(|c| c.parse::<u32>().ok());
            let ln = parts.next().and_then(|c| c.parse::<u32>().ok());
            let file = parts.next().unwrap_or(loc);
            match (file == self.file, ln, col) {
                (true, Some(ln), Some(col)) => {
                    let (sl, sc) = self.map.original(ln, col).unwrap_or((ln, col));
                    first.get_or_insert((sl, sc));
                    frames.push(format!("{func} ({file}:{sl}:{sc})"));
                }
                _ => frames.push(format!("{func} ({loc})")),
            }
        }
        (frames, first)
    }

    fn exception(&self, t: Thrown) -> ComposeError {
        let (frames, loc) = t
            .stack
            .as_deref()
            .map_or((Vec::new(), None), |s| self.map_stack(s));
        let name = t.name.clone().unwrap_or_else(|| "Error".into());
        let (kind, code, phase, message) = match t.code.as_deref() {
            Some("CW_PROPS") => (
                ErrorKind::Props,
                codes::PROPS,
                Phase::Props,
                t.message.clone(),
            ),
            Some("CW_ASYNC") => (
                ErrorKind::Runtime,
                codes::ASYNC_RENDER,
                Phase::Runtime,
                t.message.clone(),
            ),
            Some("CW_DEFINE" | "CW_EXPORT") => (
                ErrorKind::Runtime,
                codes::DEFINITION,
                Phase::Runtime,
                t.message.clone(),
            ),
            Some("CW_SCENE_TOO_LARGE") => (
                ErrorKind::Scene,
                codes::SCENE_TOO_LARGE,
                Phase::Scene,
                t.message.clone(),
            ),
            Some("CW_ELEMENT" | "CW_CHILD") => (
                ErrorKind::Scene,
                codes::SCENE_INVALID,
                Phase::Scene,
                t.message.clone(),
            ),
            _ if t.message.contains("out of memory") => (
                ErrorKind::OutOfMemory,
                codes::OUT_OF_MEMORY,
                Phase::Runtime,
                format!("{name}: out of memory (limit hit)"),
            ),
            _ if name == "RangeError" && t.message.contains("stack") => (
                ErrorKind::StackOverflow,
                codes::STACK_OVERFLOW,
                Phase::Runtime,
                format!("{name}: {}", t.message),
            ),
            _ => (
                ErrorKind::Runtime,
                codes::EXCEPTION,
                Phase::Runtime,
                format!("{name}: {}", t.message),
            ),
        };
        let mut d = Diagnostic::error(code, phase, message);
        if let Some(start) = loc {
            d = d.at(self.file, start, None);
        }
        if let Some(p) = t.path {
            d = d.with_path(p);
        }
        d.stack = frames;
        ComposeError::new(kind, d)
    }
}

/// Convenience: transpile + load + bind defaults for tests and the CLI.
pub fn load_source(file: &str, source: &str, limits: Limits) -> Result<Composition, ComposeError> {
    let compiled = crate::transpile::transpile(file, source)?;
    Composition::load(&compiled, limits)
}

/// Lists the global names visible to compositions (sandbox audit).
pub fn sandbox_globals(comp: &Composition) -> Result<Vec<String>, ComposeError> {
    comp.host().guarded(comp.limits.frame_time, |ctx| {
        let globals: Object = ctx.globals();
        let names: Vec<String> = ctx
            .eval::<Function, _>("(g) => Object.getOwnPropertyNames(g).sort()")?
            .call((globals,))?;
        Ok(names)
    })
}
