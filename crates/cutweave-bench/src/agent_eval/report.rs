// SPDX-License-Identifier: GPL-3.0-or-later
//! The result file of a run ([`RunResult`], JSON) and the Markdown report made from it.
//!
//! The report is only ever generated from the JSON (`agent-eval --report FILE` makes it
//! again), so the two cannot disagree. It answers the questions of the work package with
//! numbers: do parameter descriptions change first-call validity, what does a core tool save
//! over `cutweave_execute`, what does a batch save.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::run::{Session, VARIANTS};
use super::tasks::TASKS;
use super::transcript::Measures;

/// Everything a run produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    /// The format version ([`super::transcript::FORMAT`]).
    pub format: u32,
    /// UTC date of the run, `YYYY-MM-DD`.
    pub date: String,
    /// The model id.
    pub model: String,
    /// `output_config.effort`, when one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// The most responses a session could take.
    pub max_turns: u32,
    /// The name and SHA-256 of the project every session started from.
    pub fixture: String,
    /// How the numbers the API does not count were estimated.
    pub estimates: String,
    /// The sessions, in the order they ran.
    pub sessions: Vec<Session>,
    /// Sessions that could not be run, with the reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// The note every result file carries about its estimates.
pub const ESTIMATES: &str = "Input and output tokens are the API's own counts (usage). \
Tokens of tool results and of tool definitions are estimates: bytes of the text or JSON \
divided by 4, rounded up.";

/// Sums over a group of sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Group {
    sessions: u64,
    passed: u64,
    tools_used: u64,
    first_valid: u64,
    turns: u64,
    tool_calls: u64,
    describe: u64,
    search: u64,
    tool_search: u64,
    batches: u64,
    input: u64,
    output: u64,
    results: u64,
    definitions: u64,
}

impl Group {
    fn of<'a>(sessions: impl Iterator<Item = &'a Session>) -> Group {
        let mut g = Group::default();
        for Session { measures: m, .. } in sessions {
            g.add(m);
        }
        g
    }

    fn add(&mut self, m: &Measures) {
        self.sessions += 1;
        self.passed += u64::from(m.success);
        self.tools_used += m.tools_used;
        self.first_valid += m.first_calls_valid;
        self.turns += m.turns;
        self.tool_calls += m.tool_calls;
        self.describe += m.describe_calls;
        self.search += m.search_calls;
        self.tool_search += m.tool_search_calls;
        self.batches += m.batches;
        self.input += m.input_tokens;
        self.output += m.output_tokens;
        self.results += m.tool_result_tokens;
        self.definitions += m.tool_definition_tokens_in_context;
    }

    /// The mean of a sum per session, with one decimal.
    fn mean(&self, sum: u64) -> String {
        if self.sessions == 0 {
            return "-".to_owned();
        }
        #[allow(clippy::cast_precision_loss)]
        let mean = sum as f64 / self.sessions as f64;
        if mean >= 100.0 {
            format!("{mean:.0}")
        } else {
            format!("{mean:.1}")
        }
    }

    fn validity(&self) -> String {
        percent(self.first_valid, self.tools_used)
    }
}

/// `part` of `whole` as `7/8 (88%)`.
fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "-".to_owned();
    }
    #[allow(clippy::cast_precision_loss)]
    let share = 100.0 * part as f64 / whole as f64;
    format!("{part}/{whole} ({share:.0}%)")
}

/// The difference of two sums per session, `b` against `a`, as `-1.5 (-30%)`.
fn change(a: (u64, u64), b: (u64, u64)) -> String {
    if a.1 == 0 || b.1 == 0 {
        return "-".to_owned();
    }
    #[allow(clippy::cast_precision_loss)]
    let (mean_a, mean_b) = (a.0 as f64 / a.1 as f64, b.0 as f64 / b.1 as f64);
    let diff = mean_b - mean_a;
    if mean_a == 0.0 {
        return format!("{diff:+.1}");
    }
    format!("{diff:+.1} ({:+.0}%)", 100.0 * diff / mean_a)
}

/// The sessions of variant `a` and of variant `b` in client mode `client`, on the tasks
/// both ran to an end (no API error on either side).
fn pairs<'a>(
    run: &'a RunResult,
    a: &str,
    b: &str,
    client: &str,
) -> (Vec<&'a Session>, Vec<&'a Session>) {
    let find = |variant: &str, task: &str| {
        run.sessions.iter().find(|s| {
            s.variant == variant
                && s.client == client
                && s.task == task
                && !s.transcript.ended.starts_with("error")
        })
    };
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for task in &TASKS {
        if let (Some(x), Some(y)) = (find(a, task.id), find(b, task.id)) {
            left.push(x);
            right.push(y);
        }
    }
    (left, right)
}

/// One comparison of two variants: a table with a row per measure.
fn compare(out: &mut String, run: &RunResult, a: &str, b: &str, client: &str) {
    let (left, right) = pairs(run, a, b, client);
    if left.is_empty() {
        let _ = writeln!(
            out,
            "Not measured in this run for the `{client}` client: no task ran in both `{a}` and `{b}`.\n"
        );
        return;
    }
    let (x, y) = (Group::of(left.into_iter()), Group::of(right.into_iter()));
    let _ = writeln!(
        out,
        "`{client}` client, tasks that ran in both: {}\n",
        x.sessions
    );
    let _ = writeln!(out, "| Per task | `{a}` | `{b}` | Difference |");
    let _ = writeln!(out, "|---|---|---|---|");
    let _ = writeln!(
        out,
        "| Tasks passed | {} | {} | |",
        percent(x.passed, x.sessions),
        percent(y.passed, y.sessions)
    );
    let _ = writeln!(
        out,
        "| First call to a tool valid | {} | {} | |",
        x.validity(),
        y.validity()
    );
    let rows: [(&str, u64, u64); 8] = [
        ("Turns", x.turns, y.turns),
        ("Tool calls", x.tool_calls, y.tool_calls),
        ("Describe calls", x.describe, y.describe),
        ("Search calls", x.search, y.search),
        ("Input tokens", x.input, y.input),
        ("Output tokens", x.output, y.output),
        ("Tool result tokens (est.)", x.results, y.results),
        (
            "Tool definition tokens in context (est.)",
            x.definitions,
            y.definitions,
        ),
    ];
    for (name, sum_x, sum_y) in rows {
        let diff = change((sum_x, x.sessions), (sum_y, y.sessions));
        let _ = writeln!(
            out,
            "| {name} | {} | {} | {diff} |",
            x.mean(sum_x),
            y.mean(sum_y)
        );
    }
    let _ = writeln!(out);
}

fn clients(run: &RunResult) -> Vec<&str> {
    ["upfront", "deferred"]
        .into_iter()
        .filter(|client| run.sessions.iter().any(|s| s.client == *client))
        .collect()
}

fn summary(out: &mut String, run: &RunResult) {
    let _ = writeln!(out, "## Summary\n");
    let _ = writeln!(out, "Means are per session.\n");
    let _ = writeln!(
        out,
        "| Variant | Client | Passed | First call valid | Turns | Tool calls | Describe | Search | Tool search | Tokens in | Tokens out | Tool results (est.) | Definitions in context (est.) |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for variant in &VARIANTS {
        for client in clients(run) {
            let of = run
                .sessions
                .iter()
                .filter(|s| s.variant == variant.name && s.client == client);
            let g = Group::of(of);
            if g.sessions == 0 {
                continue;
            }
            let _ = writeln!(
                out,
                "| `{}` | {client} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                variant.name,
                percent(g.passed, g.sessions),
                g.validity(),
                g.mean(g.turns),
                g.mean(g.tool_calls),
                g.mean(g.describe),
                g.mean(g.search),
                g.mean(g.tool_search),
                g.mean(g.input),
                g.mean(g.output),
                g.mean(g.results),
                g.mean(g.definitions),
            );
        }
    }
    let _ = writeln!(out);
}

fn questions(out: &mut String, run: &RunResult) {
    let _ = writeln!(
        out,
        "## Do parameter descriptions change first-call validity?\n"
    );
    let _ = writeln!(
        out,
        "`standard` lists no parameter descriptions, `described` lists all of them; the tools, their order and what they accept are the same.\n"
    );
    for client in clients(run) {
        compare(out, run, "standard", "described", client);
    }
    let _ = writeln!(
        out,
        "## What does a core tool save over `cutweave_execute`?\n"
    );
    let _ = writeln!(
        out,
        "`via-execute` is the standard list without the task's own core tools, so the model has to find the command and run it through `cutweave_execute`.\n"
    );
    for client in clients(run) {
        compare(out, run, "via-execute", "standard", client);
    }
    let _ = writeln!(out, "## What does a batch save?\n");
    let _ = writeln!(
        out,
        "The same task, asked for as one `cutweave_execute` call (`batch`) or as one change per call (`no-batch`).\n"
    );
    for client in clients(run) {
        compare(out, run, "no-batch", "batch", client);
    }
    let _ = writeln!(out, "## MCP against the command line with a skill\n");
    let _ = writeln!(
        out,
        "Not measured: the command line path arrives with WP-2I.10 as the `cli-skill` variant of this runner.\n"
    );
}

fn per_task(out: &mut String, run: &RunResult) {
    let _ = writeln!(out, "## Every session\n");
    let _ = writeln!(
        out,
        "| Task | Variant | Client | Result | Turns | Tool calls | First call valid | Describe | Tokens in | Tokens out | Ended |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|");
    for s in &run.sessions {
        let m = &s.measures;
        let _ = writeln!(
            out,
            "| {} | `{}` | {} | {} | {} | {} | {}/{} | {} | {} | {} | {} |",
            s.task,
            s.variant,
            s.client,
            if m.success { "pass" } else { "FAIL" },
            m.turns,
            m.tool_calls,
            m.first_calls_valid,
            m.tools_used,
            m.describe_calls,
            m.input_tokens,
            m.output_tokens,
            s.transcript.ended.replace('|', "/"),
        );
    }
    let _ = writeln!(out);
    let failed: Vec<&Session> = run
        .sessions
        .iter()
        .filter(|s| !s.measures.success)
        .collect();
    if !failed.is_empty() {
        let _ = writeln!(out, "### Why sessions failed\n");
        for s in failed {
            let _ = writeln!(
                out,
                "- {} / `{}` / {}: {}",
                s.task, s.variant, s.client, s.measures.failure
            );
        }
        let _ = writeln!(out);
    }
    let invalid: Vec<String> =
        run.sessions
            .iter()
            .flat_map(|s| {
                s.measures.first_calls_invalid.iter().map(move |call| {
                    format!("- {} / `{}` / {}: `{call}`", s.task, s.variant, s.client)
                })
            })
            .collect();
    if !invalid.is_empty() {
        let _ = writeln!(out, "### First calls that failed\n");
        let _ = writeln!(
            out,
            "The code in brackets is the engine's: `invalid` means the arguments did not validate; any other code means they validated and the engine refused the call.\n"
        );
        let _ = writeln!(out, "{}\n", invalid.join("\n"));
    }
}

/// The Markdown report of `run`. `source` is the name of the JSON file it was made from.
#[must_use]
pub fn markdown(run: &RunResult, source: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<!-- Generated by `cutweave-bench agent-eval --report {source}`; do not edit. -->\n"
    );
    let _ = writeln!(out, "# Agent evaluation, {} ({})\n", run.date, run.model);
    let effort = run.effort.as_deref().unwrap_or("the model's default");
    let _ = writeln!(
        out,
        "{} sessions of model `{}` (effort: {effort}; at most {} turns each) on the project {}. Each session gives the model one task and an MCP connection to a fresh headless engine; a checker then reads the final document and says pass or fail. How to run it and how to read this page: [agent-eval.md](../agent-eval.md). The numbers and every tool call are in `{source}`.\n",
        run.sessions.len(),
        run.model,
        run.max_turns,
        run.fixture,
    );
    let _ = writeln!(out, "{}\n", run.estimates);
    let _ = writeln!(
        out,
        "One run of a model is one sample: a difference of a call or a few percent between two variants is within what a second run would change.\n"
    );
    summary(&mut out, run);
    questions(&mut out, run);
    per_task(&mut out, run);
    if !run.skipped.is_empty() {
        let _ = writeln!(out, "## Not run\n");
        for line in &run.skipped {
            let _ = writeln!(out, "- {line}");
        }
    }
    let trimmed = out.trim_end().to_owned();
    format!("{trimmed}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_eval::transcript::Transcript;

    fn session(task: &str, variant: &str, client: &str, m: Measures) -> Session {
        Session {
            task: task.to_owned(),
            variant: variant.to_owned(),
            client: client.to_owned(),
            measures: m,
            transcript: Transcript {
                ended: "end_turn".into(),
                ..Transcript::default()
            },
        }
    }

    fn measures(success: bool, used: u64, valid: u64, turns: u64, input: u64) -> Measures {
        Measures {
            success,
            failure: if success {
                String::new()
            } else {
                "A2 is not muted".into()
            },
            turns,
            tool_calls: turns - 1,
            tools_used: used,
            first_calls_valid: valid,
            first_calls_invalid: if used == valid {
                Vec::new()
            } else {
                vec!["clip_split (invalid)".into()]
            },
            input_tokens: input,
            output_tokens: 100,
            ..Measures::default()
        }
    }

    fn run() -> RunResult {
        RunResult {
            format: 1,
            date: "2026-11-28".into(),
            model: "claude-sonnet-5-5".into(),
            effort: None,
            max_turns: 20,
            fixture: "P-eval (sha256 abc)".into(),
            estimates: ESTIMATES.into(),
            sessions: vec![
                session(
                    "split-and-trim",
                    "standard",
                    "upfront",
                    measures(true, 4, 3, 5, 40_000),
                ),
                session(
                    "long-tail",
                    "standard",
                    "upfront",
                    measures(false, 4, 4, 7, 60_000),
                ),
                session(
                    "split-and-trim",
                    "described",
                    "upfront",
                    measures(true, 4, 4, 4, 60_000),
                ),
                session(
                    "long-tail",
                    "described",
                    "upfront",
                    measures(true, 4, 4, 6, 90_000),
                ),
                session(
                    "split-and-trim",
                    "batch",
                    "upfront",
                    measures(true, 3, 3, 3, 30_000),
                ),
                session(
                    "split-and-trim",
                    "no-batch",
                    "upfront",
                    measures(true, 3, 3, 6, 45_000),
                ),
            ],
            skipped: vec!["set-speed/cli-skill/upfront: arrives with WP-2I.10".into()],
        }
    }

    #[test]
    fn the_report_answers_the_questions_with_the_numbers_of_the_run() {
        let text = markdown(&run(), "2026-11-28-claude-sonnet-5-5.json");
        assert!(text.starts_with("<!-- Generated by `cutweave-bench agent-eval --report 2026-11-28-claude-sonnet-5-5.json`"));
        assert!(text.contains("# Agent evaluation, 2026-11-28 (claude-sonnet-5-5)"));
        // The summary row of the standard list: 1 of 2 passed, 7 of 8 first calls valid.
        assert!(
            text.contains("| `standard` | upfront | 1/2 (50%) | 7/8 (88%) | 6.0 | 5.0 |"),
            "{text}"
        );
        // Descriptions: 88% against 100%, and what they cost in input tokens.
        assert!(
            text.contains("| First call to a tool valid | 7/8 (88%) | 8/8 (100%) | |"),
            "{text}"
        );
        assert!(
            text.contains("| Input tokens | 50000 | 75000 | +25000.0 (+50%) |"),
            "{text}"
        );
        // A batch: 3 turns against 6.
        assert!(
            text.contains("| Turns | 6.0 | 3.0 | -3.0 (-50%) |"),
            "{text}"
        );
        // Not measured: via-execute did not run, and the command line is not built.
        assert!(text.contains("no task ran in both `via-execute` and `standard`"));
        assert!(text.contains("arrives with WP-2I.10"));
        assert!(text.contains("- long-tail / `standard` / upfront: A2 is not muted"));
        assert!(text.contains("- split-and-trim / `standard` / upfront: `clip_split (invalid)`"));
        assert!(text.contains("## Not run\n\n- set-speed/cli-skill/upfront"));
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"));
        assert!(
            !text.contains("deferred"),
            "no row for a client mode that did not run"
        );
    }

    #[test]
    fn a_session_the_api_refused_is_left_out_of_a_comparison() {
        let mut run = run();
        run.sessions[2].transcript.ended = "error: HTTP 400 invalid_request_error".into();
        let text = markdown(&run, "x.json");
        assert!(
            text.contains("`upfront` client, tasks that ran in both: 1"),
            "{text}"
        );
        // The result survives a round trip through its file.
        let json = serde_json::to_string(&run).unwrap();
        assert_eq!(serde_json::from_str::<RunResult>(&json).unwrap(), run);
    }
}
