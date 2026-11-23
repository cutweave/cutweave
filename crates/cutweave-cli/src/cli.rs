// SPDX-License-Identifier: GPL-3.0-or-later
//! The command-line surface (clap derive). `docs/reference/cli.md` is generated from it.

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

/// The parsed command line.
#[derive(Debug, Parser)]
#[command(
    name = "cutweave",
    about = "Cutweave: open-source video editor engine and command line",
    long_about = "Cutweave: open-source video editor engine and command line.\n\n\
        `exec` and `query` talk to an engine: they attach to a running one (found through its \
        run-file) or, with --headless or --project, start a one-shot engine inside the process. \
        Results are JSON on stdout; errors are JSON on stderr. Exit codes: 0 ok, 1 command \
        error, 2 usage, 3 no engine, 64 not implemented in this build.",
    disable_version_flag = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Print the cutweave version, the engine version and the command schema hash.
    #[arg(short = 'V', long)]
    pub version: bool,
    /// What to do.
    #[command(subcommand)]
    pub command: Option<Sub>,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
pub enum Sub {
    /// Run the engine process (the launch protocol Electron main and scripts speak).
    Engine(EngineCmd),
    /// Run a command, or several as one transaction, and print the result.
    Exec(ExecCmd),
    /// Run a query and print the result.
    Query(QueryCmd),
    /// List the registered commands and queries.
    Commands(CommandsCmd),
    /// Print one command's description and JSON Schemas.
    Describe(DescribeCmd),
    /// Export a sequence to a media file (needs the media backend; not available yet).
    Render(RenderCmd),
    /// Serve MCP over stdio to an agent: attach to a running engine, or run one in the process.
    #[command(
        long_about = "Serve MCP over stdio to an agent: attach to a running engine, or run one \
            in the process.\n\n\
            Stdout carries MCP protocol frames only, one JSON-RPC message per line; diagnostics \
            go to stderr and the engine's log file. Errors before the session starts are one \
            JSON object on stderr with the usual exit codes (2 usage, 3 no engine, 1 the engine \
            did not start). Closing stdin ends the session. When an attached engine goes away, \
            requests in flight get a JSON-RPC error and the process exits 3. Client setup: \
            docs/reference/mcp-clients.md."
    )]
    Mcp(McpCmd),
    /// Reserved: the sandboxed worker process.
    Worker(WorkerCmd),
}

/// How `exec`, `query`, `render` and `mcp` find their engine.
///
/// With neither `--headless` nor `--project` the command attaches to a running engine. With
/// either one (or any of the spawn-only flags) it starts a one-shot engine in the process.
#[derive(Debug, Args, Default)]
pub struct Target {
    /// Start a one-shot engine in this process instead of attaching to a running one.
    #[arg(long, help_heading = "Engine selection")]
    pub headless: bool,
    /// Open this project folder or `.cutweave` file in the spawned engine (implies --headless).
    #[arg(long, value_name = "PATH", help_heading = "Engine selection")]
    pub project: Option<PathBuf>,
    /// Attach to the engine with this process id when several are running.
    #[arg(long, value_name = "PID", help_heading = "Engine selection")]
    pub pid: Option<u32>,
    /// Directory holding the engines' run-files (default `~/.cutweave/run`).
    #[arg(long, value_name = "DIR", help_heading = "Engine selection")]
    pub run_dir: Option<PathBuf>,
    /// Seed the spawned engine's ids and freeze its clock, so identical scripts give identical
    /// output (implies --headless). `CUTWEAVE_DETERMINISTIC_IDS` does the same.
    #[arg(long, value_name = "SEED", help_heading = "Spawned engine")]
    pub deterministic_ids: Option<u64>,
    /// A folder the spawned engine lets this command import from or save into (repeatable;
    /// implies --headless). The project folder is always allowed.
    #[arg(long, value_name = "DIR", help_heading = "Spawned engine")]
    pub allow_root: Vec<PathBuf>,
    /// Dev only: fake media probing in the spawned engine (implies --headless).
    /// `CUTWEAVE_DEV_STUB_MEDIA=1` does the same.
    #[arg(long, help_heading = "Spawned engine")]
    pub dev_stub_media: bool,
}

/// `cutweave engine`: the flags of the launch protocol.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)] // independent command-line switches
pub struct EngineCmd {
    /// No UI credential: agents and the CLI use the run-file's agent tokens. Without it the
    /// engine runs in GUI mode and reads its UI credential from stdin.
    #[arg(long)]
    pub headless: bool,
    /// Bind 127.0.0.1:PORT; 0 picks a free port.
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// Accepted for compatibility: a GUI engine always reads its UI credential from stdin.
    #[arg(long)]
    pub token_stdin: bool,
    /// Open this project folder or `.cutweave` file at start.
    #[arg(long, value_name = "PATH")]
    pub project: Option<String>,
    /// Run-file directory (default `~/.cutweave/run`).
    #[arg(long, value_name = "DIR")]
    pub run_dir: Option<PathBuf>,
    /// An extra allowed `Origin` (repeatable).
    #[arg(long, value_name = "ORIGIN")]
    pub allow_origin: Vec<String>,
    /// A folder agents may import from or save into (repeatable).
    #[arg(long, value_name = "DIR")]
    pub allow_root: Vec<PathBuf>,
    /// Seeded ids and a frozen clock (tests). `CUTWEAVE_DETERMINISTIC_IDS` does the same.
    #[arg(long, value_name = "SEED")]
    pub deterministic_ids: Option<u64>,
    /// Lets a GUI engine run with deterministic ids.
    #[arg(long)]
    pub i_know_this_is_a_test: bool,
    /// Log directory (default `~/.cutweave/logs`).
    #[arg(long, value_name = "DIR", conflicts_with = "no_log")]
    pub log_dir: Option<PathBuf>,
    /// No log file.
    #[arg(long)]
    pub no_log: bool,
    /// How long a GUI engine waits for the handshake line (default 5000).
    #[arg(long, value_name = "MS")]
    pub handshake_timeout_ms: Option<u64>,
    /// Dev only: fake media probing until the FFmpeg backend lands.
    /// `CUTWEAVE_DEV_STUB_MEDIA=1` does the same.
    #[arg(long)]
    pub dev_stub_media: bool,
}

impl EngineCmd {
    /// The flags in the launch protocol's own spelling (what `EngineArgs::parse` reads).
    pub fn to_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut flag = |on: bool, name: &str| {
            if on {
                out.push(name.to_owned());
            }
        };
        flag(self.headless, "--headless");
        flag(self.token_stdin, "--token-stdin");
        flag(self.i_know_this_is_a_test, "--i-know-this-is-a-test");
        flag(self.no_log, "--no-log");
        flag(self.dev_stub_media, "--dev-stub-media");
        let mut value = |name: &str, v: Option<String>| {
            if let Some(v) = v {
                out.extend([name.to_owned(), v]);
            }
        };
        value("--port", self.port.map(|p| p.to_string()));
        value("--project", self.project.clone());
        value("--run-dir", path_arg(self.run_dir.as_ref()));
        value("--log-dir", path_arg(self.log_dir.as_ref()));
        value(
            "--deterministic-ids",
            self.deterministic_ids.map(|s| s.to_string()),
        );
        value(
            "--handshake-timeout-ms",
            self.handshake_timeout_ms.map(|s| s.to_string()),
        );
        for origin in &self.allow_origin {
            out.extend(["--allow-origin".to_owned(), origin.clone()]);
        }
        for root in &self.allow_root {
            out.extend([
                "--allow-root".to_owned(),
                root.to_string_lossy().into_owned(),
            ]);
        }
        out
    }
}

fn path_arg(path: Option<&PathBuf>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}

/// `cutweave exec`.
#[derive(Debug, Args)]
pub struct ExecCmd {
    /// The request as JSON, or `-` for stdin. One step `{"cmd": "clip.split", "args": {...}}`, or
    /// an array of steps (or `{"steps": [...]}`) that runs as one transaction and one undo step.
    #[arg(value_name = "JSON", conflicts_with = "file")]
    pub input: Option<String>,
    /// Read the request from this file (`-` for stdin).
    #[arg(short, long, value_name = "PATH")]
    pub file: Option<PathBuf>,
    /// Fail with `conflict` unless the document is at this revision.
    #[arg(long, value_name = "REV")]
    pub expected_rev: Option<u64>,
    /// After a successful run, save the project (`project.save`). A one-shot engine otherwise
    /// leaves its changes in the journal only.
    #[arg(long)]
    pub save: bool,
    /// Which engine to talk to.
    #[command(flatten)]
    pub target: Target,
}

/// `cutweave query`.
#[derive(Debug, Args)]
pub struct QueryCmd {
    /// The query name, e.g. `timeline.get`.
    pub name: String,
    /// Its params as JSON (default `{}`), or `-` for stdin.
    #[arg(value_name = "JSON")]
    pub params: Option<String>,
    /// Which engine to talk to.
    #[command(flatten)]
    pub target: Target,
}

/// `cutweave commands`.
#[derive(Debug, Args)]
pub struct CommandsCmd {
    /// Print a JSON array (name, kind, summary, scope, flags) instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// `cutweave describe`.
#[derive(Debug, Args)]
pub struct DescribeCmd {
    /// The command or query name, e.g. `clip.split`.
    pub name: String,
}

/// `cutweave render`.
#[derive(Debug, Args)]
pub struct RenderCmd {
    /// The project to render.
    #[arg(long, value_name = "PATH")]
    pub project: PathBuf,
    /// The sequence to render (`seq_...`).
    #[arg(long, value_name = "ID")]
    pub sequence: String,
    /// The output file.
    #[arg(long, value_name = "PATH")]
    pub out: PathBuf,
    /// Render only this range, `in:out` (timecode, `62.5s` or flicks).
    #[arg(long, value_name = "IN:OUT")]
    pub range: Option<String>,
    /// The export preset.
    #[arg(long, value_name = "NAME")]
    pub preset: Option<String>,
    /// Software encoding only.
    #[arg(long)]
    pub sw: bool,
    /// Seed the engine's ids (tests).
    #[arg(long, value_name = "SEED")]
    pub deterministic_ids: Option<u64>,
    /// A folder the render may read from or write to (repeatable).
    #[arg(long, value_name = "DIR")]
    pub allow_root: Vec<PathBuf>,
    /// Dev only: fake media probing.
    #[arg(long)]
    pub dev_stub_media: bool,
}

/// The MCP tool profile (arch §14).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    /// The smallest tool set.
    Minimal,
    /// The default tool set.
    Standard,
    /// Every tool.
    Full,
}

/// `cutweave mcp`.
#[derive(Debug, Args)]
pub struct McpCmd {
    /// Proxy to a running engine's MCP endpoint (`/mcp`, `/mcp/minimal` or `/mcp/full` for
    /// --profile) with its run-file's `mcp` agent token, so the agent edits the document the
    /// desktop app shows. This is the default when no spawn flag is given.
    #[arg(long, conflicts_with_all = ["headless", "project"])]
    pub attach: bool,
    /// The tool profile: the same catalogs and budgets as the engine's HTTP endpoints.
    #[arg(long, value_enum, default_value = "standard")]
    pub profile: Profile,
    /// The run-file of the engine to attach to (instead of --run-dir and --pid).
    #[arg(
        long,
        value_name = "PATH",
        requires = "attach",
        conflicts_with_all = ["pid", "run_dir"]
    )]
    pub run_file: Option<PathBuf>,
    /// Which engine to talk to.
    #[command(flatten)]
    pub target: Target,
    /// Log directory of the in-process engine (default `~/.cutweave/logs`).
    #[arg(
        long,
        value_name = "DIR",
        conflicts_with = "no_log",
        help_heading = "Spawned engine"
    )]
    pub log_dir: Option<PathBuf>,
    /// No log file for the in-process engine.
    #[arg(long, help_heading = "Spawned engine")]
    pub no_log: bool,
}

/// `cutweave worker`.
#[derive(Debug, Args)]
pub struct WorkerCmd {
    /// Ignored.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

/// The whole command tree (for help text and `docs/reference/cli.md`).
#[must_use]
pub fn command() -> clap::Command {
    Cli::command()
}
