// SPDX-License-Identifier: GPL-3.0-or-later
//! The dispatcher from the parsed command line to what each subcommand does, and the exit codes.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Parser;
use clap::error::ErrorKind;
use cutweave_api::engines::Selector;
use cutweave_commands::{EntryKind, Registry, SCHEMA_HASH};
use cutweave_engine::dev_media::env_enables;
use cutweave_engine::launch::{self, DETERMINISTIC_IDS_ENV, EXIT_USAGE as ENGINE_EXIT_USAGE};
use cutweave_engine::{DEV_STUB_MEDIA_ENV, VERSION, build_registry};
use serde_json::{Value, json};

use crate::cli::{
    Cli, CommandsCmd, DescribeCmd, EngineCmd, EnginesCmd, ExecCmd, McpCmd, QueryCmd, RenderCmd,
    Sub, Target,
};
use crate::failure::{EXIT_OK, Failure};
use crate::remote;
use crate::session::{Plan, Session, SpawnPlan, parse_request, run_request};

/// What a successful subcommand prints.
enum Out {
    /// One JSON value on a line.
    Json(Value),
    /// Plain text (the `commands` table).
    Text(String),
}

/// Runs the CLI with `args` (including the program name) and returns the exit code.
pub fn run<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = e.print();
                return EXIT_OK;
            }
            let failure = Failure::usage(e.render().to_string().trim_end());
            failure.print();
            return failure.exit;
        }
    };
    if cli.version {
        println!(
            "cutweave {} (engine {VERSION}, schema {SCHEMA_HASH})",
            env!("CARGO_PKG_VERSION")
        );
        return EXIT_OK;
    }
    let Some(sub) = cli.command else {
        return EXIT_OK;
    };
    let result = match sub {
        Sub::Engine(cmd) => return engine(&cmd),
        Sub::Exec(cmd) => exec(&cmd).map(Out::Json),
        Sub::Query(cmd) => query(&cmd).map(Out::Json),
        Sub::Engines(cmd) => engines(&cmd),
        Sub::Commands(cmd) => commands(&cmd),
        Sub::Describe(cmd) => describe(&cmd).map(Out::Json),
        Sub::Render(cmd) => render(&cmd).map(Out::Json),
        // Stdout is the protocol channel: `mcp` prints nothing else there.
        Sub::Mcp(cmd) => return mcp(&cmd),
        Sub::Worker(_) => Err(Failure::unimplemented(
            "`cutweave worker` is not implemented in this build",
        )),
    };
    match result {
        Ok(Out::Json(value)) => {
            println!("{value}");
            EXIT_OK
        }
        Ok(Out::Text(text)) => {
            print!("{text}");
            EXIT_OK
        }
        Err(failure) => {
            failure.print();
            failure.exit
        }
    }
}

/// `cutweave engine`: the launch protocol (stdout carries its JSON lines).
fn engine(cmd: &EngineCmd) -> i32 {
    let ids_env = std::env::var(DETERMINISTIC_IDS_ENV).ok();
    let stub_env = std::env::var(DEV_STUB_MEDIA_ENV).ok();
    match launch::EngineArgs::parse_with_env(cmd.to_args(), ids_env.as_deref()) {
        Ok(args) => launch::run(
            &args.with_dev_stub_env(stub_env.as_deref()),
            std::io::stdin(),
            &mut std::io::stdout(),
        ),
        Err(message) => {
            println!(
                "{}",
                json!({
                    "event": "error",
                    "code": "usage",
                    "exit_code": ENGINE_EXIT_USAGE,
                    "message": message,
                })
            );
            eprintln!("cutweave engine: {message}");
            ENGINE_EXIT_USAGE
        }
    }
}

/// How the command reaches its engine.
///
/// It attaches to a running engine, chosen by `--pid` and `--project`, or else the app whose
/// window had the focus last. `--project <path>` that no running engine has open starts a
/// one-shot engine on that path, as it always has. `--headless` and the spawn-only flags
/// always start a one-shot engine.
fn plan(target: &Target) -> Result<Plan, Failure> {
    let spawn_only = target.headless
        || target.deterministic_ids.is_some()
        || !target.allow_root.is_empty()
        || target.cache_dir.is_some()
        || target.dev_stub_media;
    if !spawn_only {
        let project = target.project.as_deref().map(str::trim);
        let fallback = match project {
            Some(p) if target.pid.is_none() && Path::new(p).exists() => Some(spawn_plan(target)?),
            _ => None,
        };
        return Ok(Plan::Attach {
            run_dir: target.run_dir.clone(),
            selector: Selector {
                pid: target.pid,
                project: project.map(str::to_owned),
            },
            fallback,
        });
    }
    if target.pid.is_some() || target.run_dir.is_some() {
        return Err(Failure::usage(
            "--pid and --run-dir attach to a running engine; they cannot be combined with \
             --headless, --deterministic-ids, --allow-root, --cache-dir or --dev-stub-media, \
             which start one",
        ));
    }
    spawn_plan(target).map(Plan::Spawn)
}

/// The one-shot engine `target` describes (`--project` is the project it opens).
fn spawn_plan(target: &Target) -> Result<SpawnPlan, Failure> {
    let seed = match target.deterministic_ids {
        Some(seed) => Some(seed),
        None => std::env::var(DETERMINISTIC_IDS_ENV)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u64>().map_err(|_| {
                    Failure::usage(format!(
                        "{DETERMINISTIC_IDS_ENV} must be an unsigned integer"
                    ))
                })
            })
            .transpose()?,
    };
    Ok(SpawnPlan {
        project: target.project.as_deref().map(PathBuf::from),
        deterministic_ids: seed,
        allow_roots: target.allow_root.clone(),
        cache_dir: target.cache_dir.clone(),
        dev_stub_media: target.dev_stub_media
            || env_enables(std::env::var(DEV_STUB_MEDIA_ENV).ok().as_deref()),
    })
}

/// `cutweave engines`: the running engines, newest focus first.
fn engines(cmd: &EnginesCmd) -> Result<Out, Failure> {
    let dir = remote::run_dir(cmd.run_dir.as_deref())?;
    let found = cutweave_api::engines::list(&dir);
    if cmd.json {
        return Ok(Out::Json(cutweave_api::engines::describe_all(&found)));
    }
    if found.is_empty() {
        return Ok(Out::Text(format!(
            "no engines running in {}\n",
            dir.display()
        )));
    }
    let mut table = format!(
        "{:<7} {:<9} {:<6} {:<20} {:<20} PROJECT\n",
        "PID", "MODE", "PORT", "STARTED", "FOCUSED"
    );
    for e in &found {
        let project = e.project.as_ref().map_or_else(
            || "(none)".to_owned(),
            |p| format!("{} ({})", p.name, p.path),
        );
        let _ = writeln!(
            table,
            "{:<7} {:<9} {:<6} {:<20} {:<20} {project}",
            e.pid,
            e.mode.as_str(),
            e.port,
            clock(e.started.saturating_mul(1000)),
            e.focused_at.map_or_else(|| "-".to_owned(), clock),
        );
    }
    Ok(Out::Text(table))
}

/// `2026-11-24 09:41:07` (UTC) for a time in milliseconds since the Unix epoch.
fn clock(millis: u64) -> String {
    let secs = millis / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The JSON text of an argument: the argument itself, or stdin for `-`.
fn json_text(arg: &str) -> Result<String, Failure> {
    if arg == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| Failure::usage(format!("cannot read stdin: {e}")))?;
        Ok(text)
    } else {
        Ok(arg.to_owned())
    }
}

fn read_file(path: &Path) -> Result<String, Failure> {
    if path == Path::new("-") {
        return json_text("-");
    }
    std::fs::read_to_string(path)
        .map_err(|e| Failure::usage(format!("cannot read {}: {e}", path.display())))
}

fn registry() -> Result<Registry, Failure> {
    build_registry().map_err(|e| Failure::error("internal", e.to_string()))
}

fn exec(cmd: &ExecCmd) -> Result<Value, Failure> {
    let text = match (&cmd.input, &cmd.file) {
        (Some(input), None) => json_text(input)?,
        (None, Some(file)) => read_file(file)?,
        _ => {
            return Err(Failure::usage(
                "give the request as JSON, `-` (stdin) or --file PATH",
            ));
        }
    };
    let request = parse_request(&text)?;
    let plan = plan(&cmd.target)?;
    let registry = registry()?;
    let mut session = Session::open(plan)?;
    let mut result = run_request(&mut session, &registry, request, cmd.expected_rev);
    if cmd.save
        && let Ok(Value::Object(map)) = &mut result
    {
        match session.call("project.save", json!({}), None, None) {
            Ok(saved) => {
                map.insert("saved".into(), saved.output);
            }
            Err(e) => result = Err(e),
        }
    }
    session.close();
    result
}

fn query(cmd: &QueryCmd) -> Result<Value, Failure> {
    let params: Value = match &cmd.params {
        Some(text) => serde_json::from_str(&json_text(text)?)
            .map_err(|e| Failure::usage(format!("the params are not valid JSON: {e}")))?,
        None => json!({}),
    };
    if !params.is_object() {
        return Err(Failure::usage("the params must be a JSON object"));
    }
    if let Some(entry) = registry()?.get(&cmd.name)
        && entry.kind() != EntryKind::Query
    {
        return Err(Failure::usage(format!(
            "`{}` is a command, not a query; use `cutweave exec`",
            cmd.name
        )));
    }
    let plan = plan(&cmd.target)?;
    let mut session = Session::open(plan)?;
    let result = session.call(&cmd.name, params, None, None);
    session.close();
    let done = result?;
    Ok(json!({"rev": done.rev, "committed_rev": done.committed_rev, "output": done.output}))
}

fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Command => "command",
        EntryKind::Query => "query",
        EntryKind::Engine => "engine",
    }
}

fn commands(cmd: &CommandsCmd) -> Result<Out, Failure> {
    let registry = registry()?;
    let mut entries: Vec<_> = registry.list().map(|e| &e.meta).collect();
    entries.sort_by_key(|m| m.name);
    if cmd.json {
        let list: Vec<Value> = entries
            .iter()
            .map(|m| {
                json!({
                    "name": m.name,
                    "kind": kind_name(m.kind),
                    "summary": m.summary,
                    "scope": m.scope,
                    "undoable": m.undoable,
                    "view": m.view,
                    "mcp": m.mcp,
                    "annotations": m.annotations,
                })
            })
            .collect();
        return Ok(Out::Json(Value::Array(list)));
    }
    let width = entries.iter().map(|m| m.name.len()).max().unwrap_or(0);
    let mut table = String::new();
    for m in &entries {
        let _ = writeln!(
            table,
            "{:<width$}  {:<7}  {}",
            m.name,
            kind_name(m.kind),
            m.summary
        );
    }
    Ok(Out::Text(table))
}

fn describe(cmd: &DescribeCmd) -> Result<Value, Failure> {
    let registry = registry()?;
    let entry = registry.get(&cmd.name).ok_or_else(|| {
        Failure::error(
            "not_found",
            format!(
                "no command or query named `{}` (`cutweave commands` lists them)",
                cmd.name
            ),
        )
    })?;
    serde_json::to_value(&entry.meta).map_err(|e| Failure::error("internal", e.to_string()))
}

fn render(cmd: &RenderCmd) -> Result<Value, Failure> {
    if let (Some(frames), Some(out_dir)) = (&cmd.frames, &cmd.out_dir) {
        return crate::render::frames(cmd, frames, out_dir);
    }
    Err(Failure::error(
        "unsupported",
        "`cutweave render` to a media file needs the media backend (WP-1I.05, WP-1E.03), which \
         this build does not have yet; `--frames` writes still frames",
    ))
}

/// `cutweave mcp`: MCP over stdio (WP-1J.03). Returns the exit code; failures before the
/// session starts are JSON on stderr.
fn mcp(cmd: &McpCmd) -> i32 {
    let checked = plan(&cmd.target).and_then(|mut plan| {
        let spawn = matches!(plan, Plan::Spawn(_));
        // `--attach` never falls back to an engine of its own.
        let mut may_spawn = spawn;
        if let Plan::Attach { fallback, .. } = &mut plan {
            if cmd.attach {
                *fallback = None;
            }
            may_spawn = fallback.is_some();
        }
        if cmd.attach && spawn {
            return Err(Failure::usage(
                "--attach proxies to a running engine; it cannot be combined with \
                 --deterministic-ids, --allow-root, --cache-dir or --dev-stub-media, which start one",
            ));
        }
        if !spawn && !may_spawn && (cmd.log_dir.is_some() || cmd.no_log) {
            return Err(Failure::usage(
                "--log-dir and --no-log configure the in-process engine of --headless; an \
                 attached engine logs on its own",
            ));
        }
        Ok(plan)
    });
    match checked {
        Ok(plan) => crate::mcp::run(cmd, plan),
        Err(failure) => {
            failure.print();
            failure.exit
        }
    }
}
