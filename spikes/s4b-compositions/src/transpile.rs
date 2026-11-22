// SPDX-License-Identifier: GPL-3.0-or-later
//! TS/TSX → JS with oxc, in-process (arch §12).
//!
//! parse → semantic (early errors) → sandbox policy checks on the module record and AST →
//! transform (TypeScript strip + automatic JSX runtime with `importSource: "@cutweave/compose"`, no
//! downlevelling: QuickJS-ng runs ES2023+) → codegen (ASCII-only, with a source map so runtime
//! errors map back to TS positions).

use std::path::Path;
use std::time::Instant;

use oxc::allocator::Allocator;
use oxc::ast::ast::{CallExpression, Expression, IdentifierReference, NewExpression};
use oxc::ast_visit::{Visit, walk};
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::diagnostics::{OxcDiagnostic, Severity as OxcSeverity};
use oxc::parser::Parser;
use oxc::semantic::{Scoping, SemanticBuilder};
use oxc::span::{SourceType, Span};
use oxc::transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer};
use serde::Serialize;

use crate::diagnostics::{ComposeError, Diagnostic, ErrorKind, LineIndex, Phase, codes};

/// The authoring module.
pub const COMPOSE_MODULE: &str = "@cutweave/compose";
/// Target of the automatic JSX transform.
pub const JSX_RUNTIME_MODULE: &str = "@cutweave/compose/jsx-runtime";

/// Globals that exist in browsers / Node / Deno but not in the sandbox. Referencing one is a
/// warning (code guarded by `typeof fetch !== "undefined"` is legitimate); calling it at runtime
/// is a `ReferenceError`.
const SANDBOX_MISSING_GLOBALS: &[&str] = &[
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "EventSource",
    "process",
    "Deno",
    "Bun",
    "window",
    "document",
    "navigator",
    "localStorage",
    "sessionStorage",
    "indexedDB",
    "caches",
    "setTimeout",
    "setInterval",
    "setImmediate",
    "queueMicrotask",
    "requestAnimationFrame",
    "importScripts",
    "Worker",
    "SharedArrayBuffer",
    "Atomics",
    "WeakRef",
    "FinalizationRegistry",
];

/// Wall-clock cost of each stage, in microseconds.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct TranspileTimings {
    pub parse_us: f64,
    pub semantic_us: f64,
    pub transform_us: f64,
    pub codegen_us: f64,
    pub total_us: f64,
}

/// Generated → original position map (from oxc's source map tokens).
#[derive(Clone, Debug, Default)]
pub struct SourceMapTable {
    /// Per generated line (0-based): `(dst_col, src_line, src_col)` sorted by `dst_col`, 0-based.
    lines: Vec<Vec<(u32, u32, u32)>>,
}

impl SourceMapTable {
    /// Maps a 1-based (line, column) in the generated JS to a 1-based position in the source.
    #[must_use]
    pub fn original(&self, line: u32, column: u32) -> Option<(u32, u32)> {
        let toks = self.lines.get(line.checked_sub(1)? as usize)?;
        let col0 = column.saturating_sub(1);
        let i = toks.partition_point(|t| t.0 <= col0);
        let t = if i == 0 { toks.first()? } else { &toks[i - 1] };
        Some((t.1 + 1, t.2 + 1))
    }
}

/// A transpiled composition.
#[derive(Clone, Debug)]
pub struct Compiled {
    /// Module name used in QuickJS (and in stack traces): the source file name.
    pub file: String,
    pub js: String,
    pub warnings: Vec<Diagnostic>,
    pub map: SourceMapTable,
    pub timings: TranspileTimings,
}

fn us(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e6
}

fn convert(d: &OxcDiagnostic, phase: Phase, file: &str, idx: &LineIndex<'_>) -> Diagnostic {
    let code = match phase {
        Phase::Parse => codes::SYNTAX,
        Phase::Semantic => codes::SEMANTIC,
        _ => codes::TRANSFORM,
    };
    let mut out = if d.severity == OxcSeverity::Error {
        Diagnostic::error(code, phase, d.message.to_string())
    } else {
        Diagnostic::warning(code, phase, d.message.to_string())
    };
    let label = d
        .labels
        .iter()
        .find(|l| l.primary())
        .or_else(|| d.labels.first());
    if let Some(l) = label {
        out = out.at(
            file,
            idx.position(l.offset()),
            Some(idx.position(l.offset() + l.len())),
        );
        if out.help.is_none()
            && let Some(text) = l.label()
        {
            out.help = Some(text.to_string());
        }
    } else {
        out.file = Some(file.to_string());
    }
    if let Some(h) = &d.help {
        out.help = Some(h.to_string());
    }
    out
}

fn located(d: Diagnostic, file: &str, idx: &LineIndex<'_>, span: Span) -> Diagnostic {
    d.at(file, idx.position(span.start), Some(idx.position(span.end)))
}

/// Finds `Function(...)` / `new Function(...)` call sites (the callee identifier spans).
/// Finds references to sandbox-relevant *global* names (unresolved references: a local
/// `require` or `eval` binding is fine) and `Function(...)` / `new Function(...)` calls.
struct GlobalRefs<'s> {
    scoping: &'s Scoping,
    /// (name, span) of interesting unresolved references.
    refs: Vec<(String, Span)>,
    function_calls: Vec<Span>,
}

impl GlobalRefs<'_> {
    fn is_global(&self, id: &IdentifierReference<'_>) -> bool {
        id.reference_id
            .get()
            .is_none_or(|r| self.scoping.get_reference(r).symbol_id().is_none())
    }

    fn callee(&mut self, callee: &Expression<'_>) {
        if let Expression::Identifier(id) = callee
            && id.name == "Function"
            && self.is_global(id)
        {
            self.function_calls.push(id.span);
        }
    }
}

impl<'a> Visit<'a> for GlobalRefs<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let name = it.name.as_str();
        if (name == "eval" || name == "require" || SANDBOX_MISSING_GLOBALS.contains(&name))
            && self.is_global(it)
        {
            self.refs.push((name.to_string(), it.span));
        }
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.callee(&it.callee);
        walk::walk_call_expression(self, it);
    }

    fn visit_new_expression(&mut self, it: &NewExpression<'a>) {
        self.callee(&it.callee);
        walk::walk_new_expression(self, it);
    }
}

/// Transpiles one composition source file. Errors carry every error-severity diagnostic.
pub fn transpile(file: &str, source: &str) -> Result<Compiled, ComposeError> {
    let t_total = Instant::now();
    let idx = LineIndex::new(source);
    let path = Path::new(file);
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::tsx());
    let source_type = source_type.with_module(true);
    let allocator = Allocator::default();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut timings = TranspileTimings::default();

    // 1. Parse.
    let t = Instant::now();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    timings.parse_us = us(t);
    for d in parsed.diagnostics.iter() {
        let d = convert(d, Phase::Parse, file, &idx);
        if d.severity == crate::diagnostics::Severity::Error {
            errors.push(d);
        } else {
            warnings.push(d);
        }
    }
    if !errors.is_empty() {
        return Err(ComposeError {
            kind: ErrorKind::Compile,
            diagnostics: errors,
        });
    }
    let mut program = parsed.program;
    let record = parsed.module_record;

    // 2. Policy: imports, dynamic import, default export.
    for (name, reqs) in &record.requested_modules {
        let name = name.as_str();
        if name == COMPOSE_MODULE || name == JSX_RUNTIME_MODULE {
            continue;
        }
        for r in reqs.iter().filter(|r| !r.is_type) {
            errors.push(located(
                Diagnostic::error(
                    codes::IMPORT_NOT_ALLOWED,
                    Phase::Policy,
                    format!("module \"{name}\" is not available in compositions"),
                )
                .with_help(format!(
                    "compositions are self-contained: only \"{COMPOSE_MODULE}\" can be imported"
                )),
                file,
                &idx,
                r.span,
            ));
        }
    }
    for d in &record.dynamic_imports {
        errors.push(located(
            Diagnostic::error(
                codes::DYNAMIC_IMPORT,
                Phase::Policy,
                "dynamic import() is not available in compositions",
            ),
            file,
            &idx,
            d.span,
        ));
    }
    let has_default = record
        .local_export_entries
        .iter()
        .chain(record.indirect_export_entries.iter())
        .any(|e| e.export_name.is_default());
    if !has_default {
        errors.push(
            Diagnostic::error(
                codes::NO_DEFAULT_EXPORT,
                Phase::Policy,
                "a composition module must `export default defineComposition({...})`",
            )
            .at(file, (1, 1), None),
        );
    }

    // 3. Semantic (early errors) + unresolved-global policy.
    let t = Instant::now();
    let sem = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .with_excess_capacity(2.0)
        .build(&program);
    for d in sem.diagnostics.iter() {
        let d = convert(d, Phase::Semantic, file, &idx);
        if d.severity == crate::diagnostics::Severity::Error {
            errors.push(d);
        } else {
            warnings.push(d);
        }
    }
    let semantic = sem.semantic;
    {
        let mut v = GlobalRefs {
            scoping: semantic.scoping(),
            refs: Vec::new(),
            function_calls: Vec::new(),
        };
        v.visit_program(&program);
        for (name, span) in v.refs {
            let (d, is_error) = match name.as_str() {
                "eval" => (
                    Diagnostic::error(
                        codes::CODE_GENERATION,
                        Phase::Policy,
                        "eval is not available in compositions",
                    )
                    .with_help("code generation from strings is disabled in the sandbox"),
                    true,
                ),
                "require" => (
                    Diagnostic::error(
                        codes::IMPORT_NOT_ALLOWED,
                        Phase::Policy,
                        "require() is not available in compositions",
                    )
                    .with_help(format!("use `import {{ ... }} from \"{COMPOSE_MODULE}\"`")),
                    true,
                ),
                _ => (
                    Diagnostic::warning(
                        codes::SANDBOX_GLOBAL,
                        Phase::Policy,
                        format!(
                            "`{name}` does not exist in the composition sandbox (no I/O, no timers)"
                        ),
                    ),
                    false,
                ),
            };
            let d = located(d, file, &idx, span);
            if is_error {
                errors.push(d);
            } else {
                warnings.push(d);
            }
        }
        for span in v.function_calls {
            errors.push(located(
                Diagnostic::error(
                    codes::CODE_GENERATION,
                    Phase::Policy,
                    "the Function constructor is not available in compositions",
                )
                .with_help("code generation from strings is disabled in the sandbox"),
                file,
                &idx,
                span,
            ));
        }
    }
    timings.semantic_us = us(t);
    if !errors.is_empty() {
        errors.sort_by_key(|d| (d.line, d.column));
        return Err(ComposeError {
            kind: ErrorKind::Compile,
            diagnostics: errors,
        });
    }
    let scoping = semantic.into_scoping();

    // 4. Transform: strip TS, automatic JSX runtime → `@cutweave/compose/jsx-runtime`.
    let t = Instant::now();
    let options = TransformOptions {
        jsx: JsxOptions {
            runtime: JsxRuntime::Automatic,
            import_source: Some(COMPOSE_MODULE.to_string()),
            pure: false,
            display_name_plugin: false,
            ..JsxOptions::enable()
        },
        ..TransformOptions::default()
    };
    let ret =
        Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
    timings.transform_us = us(t);
    for d in ret.diagnostics.iter() {
        let d = convert(d, Phase::Transform, file, &idx);
        if d.severity == crate::diagnostics::Severity::Error {
            errors.push(d);
        } else {
            warnings.push(d);
        }
    }
    if !errors.is_empty() {
        return Err(ComposeError {
            kind: ErrorKind::Compile,
            diagnostics: errors,
        });
    }

    // 5. Codegen with a source map (ASCII-only output: QuickJS columns are bytes).
    let t = Instant::now();
    let out = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(path.to_path_buf()),
            ascii_only: true,
            ..CodegenOptions::default()
        })
        .build(&program);
    let mut map = SourceMapTable::default();
    if let Some(sm) = &out.map {
        for tok in sm.get_tokens() {
            let line = tok.get_dst_line() as usize;
            if map.lines.len() <= line {
                map.lines.resize_with(line + 1, Vec::new);
            }
            map.lines[line].push((tok.get_dst_col(), tok.get_src_line(), tok.get_src_col()));
        }
        for l in &mut map.lines {
            l.sort_by_key(|t| t.0);
        }
    }
    timings.codegen_us = us(t);
    timings.total_us = us(t_total);

    Ok(Compiled {
        file: file.to_string(),
        js: out.code,
        warnings,
        map,
        timings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"
import { defineComposition, p, Rect, Group } from "@cutweave/compose";
type Props = { w: number };
export default defineComposition({
  id: "t.ok",
  props: p.object({ w: p.number().default(10) }),
  durationInFrames: 10,
  render: ({ props }: { props: Props }) => (<Group><Rect w={props.w} h={5} /></Group>),
});
"#;

    #[test]
    fn transpiles_tsx_to_jsx_runtime_calls() {
        let c = transpile("ok.tsx", OK).expect("compiles");
        assert!(c.js.contains("@cutweave/compose/jsx-runtime"), "{}", c.js);
        assert!(!c.js.contains("type Props"), "types stripped: {}", c.js);
        assert!(!c.js.contains("<Rect"), "JSX lowered: {}", c.js);
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert!(c.js.is_ascii());
    }

    #[test]
    fn syntax_error_has_location() {
        let src = "export default 1;\nconst x = (;\n";
        let e = transpile("bad.tsx", src).expect_err("syntax error");
        assert_eq!(e.kind, ErrorKind::Compile);
        let d = &e.diagnostics[0];
        assert_eq!(d.code, codes::SYNTAX);
        assert_eq!(d.file.as_deref(), Some("bad.tsx"));
        assert_eq!(d.line, Some(2));
        assert_eq!(d.column, Some(12));
    }

    #[test]
    fn foreign_imports_and_dynamic_import_are_rejected() {
        let src = "import fs from \"fs\";\nimport type { X } from \"anything\";\n\
                   export default () => import(\"./x\");\n";
        let e = transpile("imp.ts", src).expect_err("policy");
        let codes: Vec<_> = e.diagnostics.iter().map(|d| (d.code, d.line)).collect();
        assert!(
            codes.contains(&(codes::IMPORT_NOT_ALLOWED, Some(1))),
            "{codes:?}"
        );
        assert!(
            codes.contains(&(codes::DYNAMIC_IMPORT, Some(3))),
            "{codes:?}"
        );
        assert_eq!(
            codes.len(),
            2,
            "type-only import is erased and allowed: {codes:?}"
        );
    }

    #[test]
    fn eval_require_and_function_ctor_are_rejected_unless_shadowed() {
        let src = "const a = eval(\"1\");\nconst b = require(\"fs\");\nconst c = new Function(\"x\");\n\
                   function f(eval2: number) { const require = (s: string) => s; return require(\"ok\"); }\n\
                   export default [a, b, c, f];\n";
        let e = transpile("cg.ts", src).expect_err("policy");
        let mut got: Vec<_> = e.diagnostics.iter().map(|d| (d.code, d.line)).collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                (codes::IMPORT_NOT_ALLOWED, Some(2)),
                (codes::CODE_GENERATION, Some(1)),
                (codes::CODE_GENERATION, Some(3)),
            ]
        );
    }

    #[test]
    fn missing_default_export_and_io_globals() {
        let e = transpile("nodef.ts", "export const x = 1;").expect_err("no default");
        assert_eq!(e.code(), codes::NO_DEFAULT_EXPORT);
        let c = transpile(
            "warn.ts",
            "export default () => typeof fetch === \"function\" ? fetch(\"x\") : 0;",
        )
        .expect("warnings only");
        assert_eq!(c.warnings.len(), 2);
        assert!(c.warnings.iter().all(|w| w.code == codes::SANDBOX_GLOBAL));
    }

    #[test]
    fn source_map_maps_back_to_tsx() {
        let src = "type T = number;\nexport default function f(x: T) {\n  throw new Error(\"boom\");\n}\n";
        let c = transpile("m.ts", src).expect("compiles");
        let (gl, gc) =
            c.js.lines()
                .enumerate()
                .find_map(|(i, l)| l.find("throw").map(|c| (i as u32 + 1, c as u32 + 1)))
                .expect("throw in output");
        assert_eq!(c.map.original(gl, gc), Some((3, 3)));
    }
}
