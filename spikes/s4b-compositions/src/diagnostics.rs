// SPDX-License-Identifier: GPL-3.0-or-later
//! Structured diagnostics: what an agent receives from `composition.create{source}` (arch §12).
//!
//! Every failure — parse, policy, props, runtime exception, timeout, out-of-memory, invalid scene —
//! becomes a [`ComposeError`] carrying one or more [`Diagnostic`]s with a stable `code`, the
//! original TS/TSX file, a 1-based line and a 1-based column (UTF-16 code units, the LSP default).
//! Runtime positions are mapped back through the oxc source map.

use serde::Serialize;

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// Pipeline stage that produced a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// oxc parser (syntax).
    Parse,
    /// oxc semantic checks (redeclarations, early errors).
    Semantic,
    /// oxc transformer (TS/JSX lowering).
    Transform,
    /// Sandbox policy checks on the AST (imports, code generation, I/O globals).
    Policy,
    /// Props validation against the `p.*` schema.
    Props,
    /// Module evaluation or `render()` in QuickJS.
    Runtime,
    /// Converting the returned nodes into the S4 scene graph.
    Scene,
}

/// One structured diagnostic.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable code, e.g. `CW0101` (see [`codes`]).
    pub code: &'static str,
    pub phase: Phase,
    pub message: String,
    /// Source file (the composition's TS/TSX name), when the diagnostic has a location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// 1-based column in UTF-16 code units.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// JSON Pointer into the props (props errors) or a node path (scene errors).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// JS stack (runtime errors), frames mapped back to the TS source where possible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stack: Vec<String>,
}

impl Diagnostic {
    /// An error without location.
    #[must_use]
    pub fn error(code: &'static str, phase: Phase, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            phase,
            message: message.into(),
            file: None,
            line: None,
            column: None,
            end_line: None,
            end_column: None,
            path: None,
            help: None,
            stack: Vec::new(),
        }
    }

    /// A warning without location.
    #[must_use]
    pub fn warning(code: &'static str, phase: Phase, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, phase, message)
        }
    }

    /// Attaches a file and start/end positions.
    #[must_use]
    pub fn at(mut self, file: &str, start: (u32, u32), end: Option<(u32, u32)>) -> Self {
        self.file = Some(file.to_string());
        self.line = Some(start.0);
        self.column = Some(start.1);
        if let Some((l, c)) = end {
            self.end_line = Some(l);
            self.end_column = Some(c);
        }
        self
    }

    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// `file:line:col: severity[code]: message` (one line, for humans and logs).
    #[must_use]
    pub fn render(&self) -> String {
        let loc = match (&self.file, self.line, self.column) {
            (Some(f), Some(l), Some(c)) => format!("{f}:{l}:{c}: "),
            (Some(f), _, _) => format!("{f}: "),
            _ => String::new(),
        };
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let path = self
            .path
            .as_ref()
            .map_or(String::new(), |p| format!(" (at {p})"));
        let help = self
            .help
            .as_ref()
            .map_or(String::new(), |h| format!(" — help: {h}"));
        format!("{loc}{sev}[{}]: {}{path}{help}", self.code, self.message)
    }
}

/// Stable diagnostic codes.
pub mod codes {
    pub const SYNTAX: &str = "CW0001";
    pub const SEMANTIC: &str = "CW0002";
    pub const TRANSFORM: &str = "CW0003";
    pub const IMPORT_NOT_ALLOWED: &str = "CW0101";
    pub const DYNAMIC_IMPORT: &str = "CW0102";
    pub const CODE_GENERATION: &str = "CW0103";
    pub const SANDBOX_GLOBAL: &str = "CW0104";
    pub const NO_DEFAULT_EXPORT: &str = "CW0105";
    pub const PROPS: &str = "CW0201";
    pub const EXCEPTION: &str = "CW0301";
    pub const TIMEOUT: &str = "CW0302";
    pub const OUT_OF_MEMORY: &str = "CW0303";
    pub const STACK_OVERFLOW: &str = "CW0304";
    pub const ASYNC_RENDER: &str = "CW0305";
    pub const DEFINITION: &str = "CW0306";
    pub const IMPURE: &str = "CW0307";
    pub const SCENE_INVALID: &str = "CW0401";
    pub const SCENE_TOO_LARGE: &str = "CW0402";
    pub const ENGINE: &str = "CW0900";
}

/// Error class (what the host should do about it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Source does not compile or violates the sandbox policy: fix the code.
    Compile,
    /// Props do not match the schema: fix the props.
    Props,
    /// The composition threw.
    Runtime,
    /// Interrupted by the per-frame (or load) time limit.
    Timeout,
    /// Hit the runtime memory limit.
    OutOfMemory,
    /// Exceeded the JS stack limit.
    StackOverflow,
    /// `render()` returned something that is not a valid scene.
    Scene,
    /// Host / engine failure (not the composition's fault).
    Engine,
}

/// A failed compile, load or frame evaluation.
#[derive(Clone, Debug, Serialize, thiserror::Error)]
#[error("{kind:?}: {}", .diagnostics.first().map_or_else(String::new, Diagnostic::render))]
pub struct ComposeError {
    pub kind: ErrorKind,
    pub diagnostics: Vec<Diagnostic>,
}

impl ComposeError {
    #[must_use]
    pub fn new(kind: ErrorKind, diagnostic: Diagnostic) -> Self {
        Self {
            kind,
            diagnostics: vec![diagnostic],
        }
    }

    /// The first diagnostic's code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.diagnostics.first().map_or("", |d| d.code)
    }

    /// Engine failure with a message.
    #[must_use]
    pub fn engine(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::Engine,
            Diagnostic::error(codes::ENGINE, Phase::Runtime, message),
        )
    }

    /// JSON (for MCP results / CLI `--json`).
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| format!("{self}"))
    }
}

/// Byte offset → (1-based line, 1-based UTF-16 column).
pub struct LineIndex<'s> {
    source: &'s str,
    /// Byte offset of each line start.
    starts: Vec<usize>,
}

impl<'s> LineIndex<'s> {
    #[must_use]
    pub fn new(source: &'s str) -> Self {
        let mut starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { source, starts }
    }

    /// (line, column), both 1-based; column counts UTF-16 code units.
    #[must_use]
    pub fn position(&self, offset: u32) -> (u32, u32) {
        let off = (offset as usize).min(self.source.len());
        let line = self.starts.partition_point(|&s| s <= off).saturating_sub(1);
        let start = self.starts[line];
        let col: usize = self
            .source
            .get(start..off)
            .map_or(off - start, |s| s.chars().map(char::len_utf16).sum());
        (to_u32(line + 1), to_u32(col + 1))
    }
}

fn to_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_counts_utf16_columns() {
        let src = "ab\n€x\n\n🎬y";
        let idx = LineIndex::new(src);
        assert_eq!(idx.position(0), (1, 1));
        assert_eq!(idx.position(1), (1, 2));
        assert_eq!(idx.position(3), (2, 1));
        // '€' is 3 bytes / 1 UTF-16 unit.
        assert_eq!(idx.position(6), (2, 2));
        assert_eq!(idx.position(8), (3, 1));
        // '🎬' is 4 bytes / 2 UTF-16 units.
        assert_eq!(idx.position(13), (4, 3));
    }

    #[test]
    fn render_is_one_line() {
        let d = Diagnostic::error(codes::PROPS, Phase::Props, "required")
            .with_path("/name")
            .with_help("pass props.name");
        assert_eq!(
            d.render(),
            "error[CW0201]: required (at /name) — help: pass props.name"
        );
        let d = Diagnostic::warning(codes::SANDBOX_GLOBAL, Phase::Policy, "x").at(
            "a.tsx",
            (3, 7),
            None,
        );
        assert_eq!(d.render(), "a.tsx:3:7: warning[CW0104]: x");
    }
}
