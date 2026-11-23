// SPDX-License-Identifier: GPL-3.0-or-later
//! A failed invocation: the JSON printed on stderr and the process exit code.

use cutweave_commands::CmdError;
use serde_json::{Value, json};

/// Success.
pub const EXIT_OK: i32 = 0;
/// A command or query failed (the engine's `CmdError`, or a CLI-level failure).
pub const EXIT_COMMAND: i32 = 1;
/// Bad arguments or input.
pub const EXIT_USAGE: i32 = 2;
/// No engine to attach to (none running, several running, or the connection was lost).
pub const EXIT_NO_ENGINE: i32 = 3;
/// A subcommand that is reserved and not built yet (`worker`, `mcp`).
pub const EXIT_UNIMPLEMENTED: i32 = 64;

/// Why an invocation failed.
#[derive(Debug)]
pub struct Failure {
    /// The error object printed on stderr: always has `code` and `message`.
    pub json: Value,
    /// The process exit code.
    pub exit: i32,
}

impl Failure {
    /// Bad arguments or input (exit 2).
    pub fn usage(message: impl Into<String>) -> Failure {
        Failure {
            json: json!({"code": "usage", "message": message.into()}),
            exit: EXIT_USAGE,
        }
    }

    /// A CLI-level failure with its own machine `code` (exit 1).
    pub fn error(code: &str, message: impl Into<String>) -> Failure {
        Failure {
            json: json!({"code": code, "message": message.into()}),
            exit: EXIT_COMMAND,
        }
    }

    /// No engine to talk to (exit 3).
    pub fn no_engine(code: &str, message: impl Into<String>, extra: Value) -> Failure {
        let mut json = json!({"code": code, "message": message.into()});
        if let (Value::Object(json), Value::Object(extra)) = (&mut json, extra) {
            json.extend(extra);
        }
        Failure {
            json,
            exit: EXIT_NO_ENGINE,
        }
    }

    /// A reserved subcommand (exit 64).
    pub fn unimplemented(message: impl Into<String>) -> Failure {
        Failure {
            json: json!({"code": "unsupported", "message": message.into()}),
            exit: EXIT_UNIMPLEMENTED,
        }
    }

    /// The engine's error in its wire form (`code`, `message`, and the fields of that code).
    pub fn from_cmd(error: &CmdError) -> Failure {
        let json = serde_json::to_value(error)
            .unwrap_or_else(|_| json!({"code": "internal", "message": error.message()}));
        Failure::wire(json)
    }

    /// An error object received over `/rpc` (`error.data`); a connection that was lost is exit 3.
    pub fn wire(json: Value) -> Failure {
        let lost = matches!(
            json.get("code").and_then(Value::as_str),
            Some("connection_lost")
        );
        Failure {
            json,
            exit: if lost { EXIT_NO_ENGINE } else { EXIT_COMMAND },
        }
    }

    /// Adds the 0-based index of the failing step of a batch.
    #[must_use]
    pub fn at_step(mut self, step: usize) -> Failure {
        if let Value::Object(map) = &mut self.json {
            map.insert("failed_step".into(), json!(step));
        }
        self
    }

    /// Prints the error on stderr (one JSON line).
    pub fn print(&self) {
        eprintln!("{}", self.json);
    }
}
