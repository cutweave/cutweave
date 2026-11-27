// SPDX-License-Identifier: GPL-3.0-or-later
//! The agent evaluation (WP-2I.09, ADR-0040 question 4): how well does a model use
//! Cutweave's tools, measured on demand.
//!
//! `cutweave-bench agent-eval` gives a model one task at a time and an MCP connection to a
//! fresh headless engine on a seeded project, records every tool call, lets a deterministic
//! checker read the final document, and writes a dated result file (JSON) with a Markdown
//! report under `docs/reference/agent-eval/`. Variants of the same task are compared in one
//! run: the standard tool list against the described one, a core tool against the same
//! command through `cutweave_execute`, one batch against several calls, and two client
//! modes (every definition upfront, or deferred behind the API's tool search).
//!
//! It needs a model and a key, so **it never runs in the gate or in CI**: without
//! `ANTHROPIC_API_KEY` it prints how to run it and exits 0. What does run in the gate is
//! `--replay`: a recorded session's calls made again on a real engine, with no model and no
//! network (`tests/agent_eval.rs`), so the runner and the checkers cannot rot.
//!
//! - [`tasks`]: the ten tasks and their checkers.
//! - [`surface`]: the tools a model is given ([`surface::ToolSurface`]; MCP today, the
//!   command line with a skill in WP-2I.10) and the engine of a session.
//! - [`anthropic`]: the Messages API request and response shapes, and the one HTTPS POST
//!   (made by `scripts/agent-eval-post.py`: the workspace has no HTTP client with TLS).
//! - [`run`]: the session loop, the variants, and the replay.
//! - [`transcript`]: the record of a session and the measures computed from it.
//! - [`report`]: the result file and the report generated from it.
//!
//! How to run it and how to read the report: `docs/reference/agent-eval.md`.

pub mod anthropic;
pub mod report;
pub mod run;
pub mod surface;
pub mod tasks;
pub mod transcript;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anthropic::{
    ClientMode, DEFAULT_MAX_TOKENS, DEFAULT_MODEL, KEY_ENV, MESSAGES_URL, PythonTransport,
    RequestShape, ToolSearch, build_request, dry_run_text,
};
use report::{ESTIMATES, RunResult, markdown};
use run::{Bench, Plan, Stage, VARIANTS, replay, run_session, system_prompt, variant};
use surface::ToolSurface as _;
use tasks::{TASKS, task};
use transcript::{FORMAT, Transcript};

use crate::BenchError;
use crate::result::iso_date;

/// `cutweave-bench agent-eval --help`.
pub const USAGE: &str = "usage: cutweave-bench agent-eval [options]
  Gives a model the evaluation's tasks over MCP on a fresh headless engine and writes a dated
  result (JSON) and report (Markdown) to docs/reference/agent-eval/. Needs ANTHROPIC_API_KEY
  in the environment; without it this prints how to run it and exits 0. Never part of the gate.

  --model ID            the model (default claude-sonnet-5-5); any id the API accepts
  --tasks a,b           only these tasks (default: all ten; --list names them)
  --variants a,b        only these variants (default: all that are built)
  --client MODE         upfront | deferred | both: how the tool definitions reach the model.
                        Default: upfront for every variant, and deferred as well for
                        `standard` and `described`
  --effort LEVEL        output_config.effort (default: the model's own default)
  --max-turns N         responses a session may take (default 20)
  --max-tokens N        max_tokens of a request (default 16000)
  --tool-search KIND    bm25 | regex: the API's search tool in the deferred mode (default bm25)
  --out DIR             where the result and the report go (default docs/reference/agent-eval)
  --scratch DIR         the engines' project and run files (default target/tmp/agent-eval)
  --api-url URL         another Messages endpoint (https only)
  --dry-run             print the first request it would send, the key redacted; sends nothing
  --list                print the tasks, the variants and the sessions of a default run

  --replay FILE         make a recorded session's calls again on a real engine, with no model,
                        run the checker and print the measures (exit 1 when it fails)
  --record FILE --out FILE
                        the same for a session written by hand, and write the transcript
  --report FILE         write the Markdown report of a result file again, next to it";

/// What to say when there is no key.
pub const HOW_TO_RUN: &str = "agent-eval: ANTHROPIC_API_KEY is not set, so nothing was run.

The evaluation gives a model the ten tasks over MCP and measures how it uses the tools. It
needs an Anthropic API key in the environment and costs real money: a default run is
57 sessions (docs/reference/agent-eval.md has the estimate). To run it:

  ANTHROPIC_API_KEY=... cargo run -p cutweave-bench -- agent-eval

  --dry-run   shows the first request without sending anything
  --list      shows the tasks, the variants and the sessions of a run
  --help      shows every option
";

/// The arguments of `cutweave-bench agent-eval`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalArgs {
    /// `--model`.
    pub model: Option<String>,
    /// `--tasks`.
    pub tasks: Option<Vec<String>>,
    /// `--variants`.
    pub variants: Option<Vec<String>>,
    /// `--client`: the modes every chosen variant runs in.
    pub clients: Option<Vec<ClientMode>>,
    /// `--effort`.
    pub effort: Option<String>,
    /// `--max-turns`.
    pub max_turns: Option<u32>,
    /// `--max-tokens`.
    pub max_tokens: Option<u32>,
    /// `--tool-search regex` (else BM25).
    pub regex_search: bool,
    /// `--out`.
    pub out: Option<PathBuf>,
    /// `--scratch`.
    pub scratch: Option<PathBuf>,
    /// `--api-url`.
    pub api_url: Option<String>,
    /// `--dry-run`.
    pub dry_run: bool,
    /// `--list`.
    pub list: bool,
    /// `--replay`.
    pub replay: Option<PathBuf>,
    /// `--record`.
    pub record: Option<PathBuf>,
    /// `--report`.
    pub report: Option<PathBuf>,
}

fn spec(why: impl Into<String>) -> BenchError {
    BenchError::Spec(why.into())
}

fn list_of(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

impl EvalArgs {
    /// Parses the arguments after `agent-eval`.
    pub fn parse(args: &[String]) -> Result<EvalArgs, BenchError> {
        let mut out = EvalArgs::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || {
                it.next()
                    .cloned()
                    .ok_or_else(|| spec(format!("{flag} needs a value")))
            };
            let number = |text: String| {
                text.parse::<u32>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| spec(format!("{flag}: `{text}` is not a positive number")))
            };
            match flag.as_str() {
                "--model" => out.model = Some(value()?),
                "--tasks" => out.tasks = Some(list_of(&value()?)),
                "--variants" => out.variants = Some(list_of(&value()?)),
                "--client" => {
                    let mode = value()?;
                    out.clients = Some(match (mode.as_str(), ClientMode::parse(&mode)) {
                        ("both", _) => vec![ClientMode::Upfront, ClientMode::Deferred],
                        (_, Some(mode)) => vec![mode],
                        _ => return Err(spec(format!("unknown client mode `{mode}`"))),
                    });
                }
                "--effort" => out.effort = Some(value()?),
                "--max-turns" => out.max_turns = Some(number(value()?)?),
                "--max-tokens" => out.max_tokens = Some(number(value()?)?),
                "--tool-search" => {
                    let kind = value()?;
                    let search = ToolSearch::parse(&kind)
                        .ok_or_else(|| spec(format!("unknown tool search `{kind}`")))?;
                    out.regex_search = search == ToolSearch::Regex;
                }
                "--out" => out.out = Some(PathBuf::from(value()?)),
                "--scratch" => out.scratch = Some(PathBuf::from(value()?)),
                "--api-url" => out.api_url = Some(value()?),
                "--dry-run" => out.dry_run = true,
                "--list" => out.list = true,
                "--replay" => out.replay = Some(PathBuf::from(value()?)),
                "--record" => out.record = Some(PathBuf::from(value()?)),
                "--report" => out.report = Some(PathBuf::from(value()?)),
                other => return Err(spec(format!("unknown option `{other}`"))),
            }
        }
        if out.record.is_some() && out.out.is_none() {
            return Err(spec("--record needs --out FILE for the transcript"));
        }
        Ok(out)
    }

    /// The sessions these arguments ask for, and the ones asked for that cannot run.
    pub fn plans(&self) -> Result<(Vec<Plan>, Vec<String>), BenchError> {
        let tasks = match &self.tasks {
            None => TASKS.iter().collect(),
            Some(names) => names
                .iter()
                .map(|name| task(name).ok_or_else(|| spec(format!("unknown task `{name}`"))))
                .collect::<Result<Vec<_>, _>>()?,
        };
        let variants = match &self.variants {
            None => VARIANTS.iter().filter(|v| v.available).collect(),
            Some(names) => names
                .iter()
                .map(|name| variant(name).ok_or_else(|| spec(format!("unknown variant `{name}`"))))
                .collect::<Result<Vec<_>, _>>()?,
        };
        let (mut plans, mut skipped) = (Vec::new(), Vec::new());
        for variant in variants {
            if !variant.available {
                skipped.push(format!("`{}`: {}", variant.name, variant.what));
                continue;
            }
            // Deferring definitions is what the described list is for (ADR-0040), so by
            // default the two lists are compared in both modes and the rest upfront.
            let lists = matches!(variant.name, "standard" | "described");
            let clients = match &self.clients {
                Some(clients) => clients.clone(),
                None if lists => vec![ClientMode::Upfront, ClientMode::Deferred],
                None => vec![ClientMode::Upfront],
            };
            for client in clients {
                for task in tasks.iter().filter(|task| variant.applies_to(task)) {
                    plans.push(Plan {
                        task,
                        variant,
                        client,
                    });
                }
            }
        }
        if plans.is_empty() {
            return Err(spec(
                "no session to run: no chosen variant applies to a chosen task",
            ));
        }
        Ok((plans, skipped))
    }

    fn shape(&self, mode: ClientMode) -> RequestShape {
        RequestShape {
            model: self
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
            max_tokens: self.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            effort: self.effort.clone(),
            mode,
            search: if self.regex_search {
                ToolSearch::Regex
            } else {
                ToolSearch::Bm25
            },
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BenchError> {
    let text = std::fs::read_to_string(path).map_err(|e| BenchError::io("read", path, e))?;
    serde_json::from_str(&text).map_err(|e| BenchError::Other(format!("{}: {e}", path.display())))
}

fn write_text(path: &Path, text: &str) -> Result<(), BenchError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| BenchError::io("create", parent, e))?;
    }
    std::fs::write(path, text).map_err(|e| BenchError::io("write", path, e))
}

fn pretty<T: serde::Serialize>(value: &T) -> Result<String, BenchError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|e| BenchError::Other(format!("cannot write JSON: {e}")))?;
    text.push('\n');
    Ok(text)
}

fn list_text(args: &EvalArgs) -> Result<String, BenchError> {
    let mut out = String::from("Tasks:\n");
    for task in &TASKS {
        let _ = writeln!(out, "  {:<20} {}", task.id, task.title);
    }
    out.push_str("\nVariants:\n");
    for variant in &VARIANTS {
        let _ = writeln!(out, "  {:<12} {}", variant.name, variant.what);
    }
    let (plans, _) = args.plans()?;
    let _ = writeln!(out, "\nSessions of this run: {}", plans.len());
    for plan in &plans {
        let _ = writeln!(out, "  {}", plan.label());
    }
    Ok(out)
}

/// `--report FILE`: the Markdown of a result file, written next to it.
fn report_again(file: &Path) -> Result<String, BenchError> {
    let run: RunResult = read_json(file)?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned());
    let target = file.with_extension("md");
    write_text(&target, &markdown(&run, &name.unwrap_or_default()))?;
    Ok(format!("wrote {}\n", target.display()))
}

/// `--replay FILE` and `--record FILE --out FILE`.
fn replay_file(
    bench: &Bench<'_>,
    file: &Path,
    write_to: Option<&Path>,
) -> Result<(String, bool), BenchError> {
    let recorded: Transcript = read_json(file)?;
    let replayed = replay(bench, &recorded)?;
    let m = &replayed.session.measures;
    let mut out = format!(
        "{} ({} / {}): {}\n",
        replayed.session.task,
        replayed.session.variant,
        replayed.session.client,
        if m.success {
            "PASS".to_owned()
        } else {
            format!("FAIL: {}", m.failure)
        },
    );
    out.push_str(&pretty(m)?);
    for difference in &replayed.differences {
        let _ = writeln!(out, "differs from the record: {difference}");
    }
    if let Some(target) = write_to {
        write_text(target, &pretty(&replayed.session.transcript)?)?;
        let _ = writeln!(out, "wrote {}", target.display());
        return Ok((out, m.success));
    }
    Ok((out, m.success && replayed.differences.is_empty()))
}

/// `--dry-run`: the first request of the first session, with a real engine's tool list.
fn dry_run(bench: &Bench<'_>, args: &EvalArgs, key_is_set: bool) -> Result<String, BenchError> {
    let (plans, _) = args.plans()?;
    let plan = plans[0];
    let stage = Stage::open(bench, &plan)?;
    let body = build_request(
        &args.shape(plan.client),
        &system_prompt(stage.surface.instructions()),
        &stage.shown,
        &[serde_json::json!({"role": "user", "content": stage.prompt})],
    );
    let url = args.api_url.as_deref().unwrap_or(MESSAGES_URL);
    Ok(format!(
        "# The first request of {} (session 1 of {}); nothing was sent.\n{}",
        plan.label(),
        plans.len(),
        dry_run_text(url, key_is_set, &body)
    ))
}

/// The first file name `<date>-<model>[-n].json` in `dir` that is not taken.
fn result_path(dir: &Path, date: &str, model: &str) -> PathBuf {
    let model: String = model
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let mut path = dir.join(format!("{date}-{model}.json"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{date}-{model}-{n}.json"));
        n += 1;
    }
    path
}

/// A run with a model: every planned session, then the result file and the report.
fn run_all(root: &Path, bench: &Bench<'_>, args: &EvalArgs) -> Result<(String, bool), BenchError> {
    let (plans, mut skipped) = args.plans()?;
    let url = args.api_url.as_deref().unwrap_or(MESSAGES_URL);
    let mut transport = PythonTransport::new(root, url, Duration::from_secs(600))?;
    let max_turns = args.max_turns.unwrap_or(20);
    let mut sessions = Vec::new();
    let mut plans = plans.into_iter();
    while let Some(plan) = plans.next() {
        eprintln!("agent-eval: {} ...", plan.label());
        let shape = args.shape(plan.client);
        let (session, fatal) = run_session(bench, &plan, &shape, max_turns, &mut transport)?;
        let m = &session.measures;
        eprintln!(
            "agent-eval:   {} in {} turns, {} tokens in, {} out ({})",
            if m.success { "pass" } else { "FAIL" },
            m.turns,
            m.input_tokens,
            m.output_tokens,
            session.transcript.ended,
        );
        if fatal {
            if sessions.is_empty() {
                return Err(BenchError::Other(format!(
                    "the API refused the key ({}); nothing was written",
                    session.transcript.ended
                )));
            }
            skipped.push(format!("{}: {}", plan.label(), session.transcript.ended));
            skipped.extend(
                plans
                    .by_ref()
                    .map(|p| format!("{}: not run after that", p.label())),
            );
            break;
        }
        sessions.push(session);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let fixture = crate::fixture::generate(&surface::eval_spec())?;
    let result = RunResult {
        format: FORMAT,
        date: iso_date(secs),
        model: args.shape(ClientMode::Upfront).model,
        effort: args.effort.clone(),
        max_turns,
        fixture: format!(
            "{} (sha256 {})",
            fixture.manifest.spec.name,
            fixture.sha256()
        ),
        estimates: ESTIMATES.to_owned(),
        sessions,
        skipped,
    };
    let dir = args
        .out
        .clone()
        .unwrap_or_else(|| root.join("docs").join("reference").join("agent-eval"));
    let json = result_path(&dir, &result.date, &result.model);
    write_text(&json, &pretty(&result)?)?;
    let name = json.file_name().map(|n| n.to_string_lossy().into_owned());
    let report = json.with_extension("md");
    write_text(&report, &markdown(&result, &name.unwrap_or_default()))?;
    let passed = result
        .sessions
        .iter()
        .filter(|s| s.measures.success)
        .count();
    let text = format!(
        "{passed} of {} sessions passed\nresult: {}\nreport: {}\n",
        result.sessions.len(),
        json.display(),
        report.display()
    );
    // The exit code says whether the run was made, not whether the model did well.
    Ok((text, true))
}

/// `cutweave-bench agent-eval`: runs in the checkout at `root` with the engine of `exe`,
/// and returns what to print and whether to exit 0.
pub fn command(root: &Path, exe: &Path, args: &[String]) -> Result<(String, bool), BenchError> {
    let args = EvalArgs::parse(args)?;
    if args.list {
        return list_text(&args).map(|text| (text, true));
    }
    if let Some(file) = &args.report {
        return report_again(file).map(|text| (text, true));
    }
    let key_is_set = std::env::var_os(KEY_ENV).is_some_and(|key| !key.is_empty());
    let offline = args.replay.is_some() || args.record.is_some() || args.dry_run;
    if !offline && !key_is_set {
        return Ok((HOW_TO_RUN.to_owned(), true));
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| BenchError::Other(format!("cannot start the runtime: {e}")))?;
    let scratch = args
        .scratch
        .clone()
        .unwrap_or_else(|| root.join("target").join("tmp").join("agent-eval"));
    let rt = Arc::new(rt);
    let bench = Bench {
        exe,
        scratch: &scratch,
        rt: &rt,
    };
    if let Some(file) = &args.replay {
        return replay_file(&bench, file, None);
    }
    if let Some(file) = &args.record {
        return replay_file(&bench, file, args.out.as_deref());
    }
    if args.dry_run {
        return dry_run(&bench, &args, key_is_set).map(|text| (text, true));
    }
    run_all(root, &bench, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<EvalArgs, BenchError> {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        EvalArgs::parse(&args)
    }

    #[test]
    fn the_arguments_of_the_command() {
        let plain = parse("").unwrap();
        assert_eq!(plain, EvalArgs::default());
        let shape = plain.shape(ClientMode::Upfront);
        assert_eq!(
            (shape.model.as_str(), shape.max_tokens),
            ("claude-sonnet-5-5", 16_000)
        );
        assert_eq!((shape.effort, shape.search), (None, ToolSearch::Bm25));
        let full = parse(
            "--model claude-fable-5-2 --tasks set-speed,long-tail --variants standard --client both \
             --effort low --max-turns 8 --max-tokens 4000 --tool-search regex --out o --scratch s \
             --api-url https://example.test/v1/messages --dry-run",
        )
        .unwrap();
        assert_eq!(full.model.as_deref(), Some("claude-fable-5-2"));
        assert_eq!(
            full.clients,
            Some(vec![ClientMode::Upfront, ClientMode::Deferred])
        );
        assert_eq!((full.max_turns, full.max_tokens), (Some(8), Some(4000)));
        assert!(full.regex_search && full.dry_run);
        assert_eq!(full.plans().unwrap().0.len(), 4);
        assert_eq!(full.shape(ClientMode::Deferred).search, ToolSearch::Regex);
        assert!(parse("--replay f.json").is_ok_and(|a| a.replay.is_some()));
        assert!(parse("--record f.json --out t.json").is_ok());
        for bad in [
            "--model",
            "--client sometimes",
            "--max-turns 0",
            "--max-tokens many",
            "--tool-search embeddings",
            "--record f.json",
            "--frobnicate",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("--tasks nope").unwrap().plans().is_err());
        assert!(parse("--variants nope").unwrap().plans().is_err());
    }

    #[test]
    fn a_default_run_is_57_sessions() {
        let (plans, skipped) = parse("").unwrap().plans().unwrap();
        // standard and described in both client modes (40), via-execute on the nine tasks
        // with a core tool, batch and no-batch on the four tasks with two changes.
        assert_eq!(plans.len(), 40 + 9 + 4 + 4);
        assert!(skipped.is_empty());
        let labels: Vec<String> = plans.iter().map(Plan::label).collect();
        assert_eq!(labels[0], "split-and-trim/standard/upfront");
        assert!(labels.contains(&"long-tail/described/deferred".to_owned()));
        assert!(
            !labels
                .iter()
                .any(|l| l.starts_with("long-tail/via-execute"))
        );
        assert!(!labels.iter().any(|l| l.ends_with("/batch/deferred")));
        assert!(HOW_TO_RUN.contains("57 sessions"));
        // One client mode for everything, when asked for.
        let deferred = parse("--client deferred").unwrap().plans().unwrap().0;
        assert_eq!(deferred.len(), 10 + 10 + 9 + 4 + 4);
        // A variant that is not built is named, not run.
        let (plans, skipped) = parse("--variants standard,cli-skill --tasks set-speed")
            .unwrap()
            .plans()
            .unwrap();
        assert_eq!((plans.len(), skipped.len()), (2, 1));
        assert!(skipped[0].contains("WP-2I.10"));
        assert!(parse("--variants cli-skill").unwrap().plans().is_err());
        assert!(
            parse("--variants batch --tasks set-speed")
                .unwrap()
                .plans()
                .is_err()
        );
    }

    #[test]
    fn the_list_and_the_result_file_name() {
        let text = list_text(&parse("--tasks long-tail --variants standard").unwrap()).unwrap();
        assert!(text.contains("  split-and-trim       Split a clip and trim the new part"));
        assert!(text.contains("  described    the described list (/mcp/described)"));
        assert!(text.contains(
            "Sessions of this run: 2\n  long-tail/standard/upfront\n  long-tail/standard/deferred\n"
        ));
        let dir = Path::new("docs").join("none-here");
        assert_eq!(
            result_path(&dir, "2026-11-28", "claude-sonnet-5-5"),
            dir.join("2026-11-28-claude-sonnet-5-5.json")
        );
        assert_eq!(
            result_path(&dir, "2026-11-28", "vendor/model:x"),
            dir.join("2026-11-28-vendor-model-x.json")
        );
    }
}
