// SPDX-License-Identifier: GPL-3.0-or-later
//! Choosing among several running engines (WP-1J.04; arch §2, §6).
//!
//! The app, scripts and `cutweave engine --headless` each run an engine, and each writes a
//! run-file (`runfile`). An agent says which one it means by project (`--project`) or process
//! id (`--pid`); when it says nothing, it gets the engine the person is looking at: the GUI
//! engine whose window had the focus last. The CLI (`exec`, `query`, `engines`, `mcp --attach`)
//! and the attach proxy's `engines_list` and `engine_attach` tools share these rules.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::runfile::{RunFile, RunFileContents, RunMode};

/// Which engine the caller means.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selector {
    /// The engine's process id.
    pub pid: Option<u32>,
    /// A project name (case-insensitive) or a project folder or document path.
    pub project: Option<String>,
}

impl Selector {
    /// Whether neither a pid nor a project is given.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pid.is_none() && self.project.is_none()
    }
}

/// Why no engine was chosen. Every case is "exit 3" for the CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectError {
    /// No engine is running.
    NoEngine,
    /// The selector names no running engine.
    NoMatch,
    /// Several headless engines run and the caller did not say which.
    Ambiguous,
}

/// Newest focus first: engines whose window focused last, then GUI before headless, then the
/// newest start. This is the order `cutweave engines` prints.
pub fn sort_by_focus(engines: &mut [RunFileContents]) {
    engines.sort_by_key(|e| {
        (
            Reverse(e.focused_at.unwrap_or(0)),
            Reverse(e.mode == RunMode::Gui),
            Reverse(e.started),
            Reverse(e.pid),
        )
    });
}

/// The running engines in `dir`, newest focus first.
#[must_use]
pub fn list(dir: &Path) -> Vec<RunFileContents> {
    let mut found = RunFile::discover(dir);
    sort_by_focus(&mut found);
    found
}

/// Picks the engine `selector` names from `engines` (any order).
///
/// - `pid`: that engine. `project`: the engine with that project open; several matches (a
///   headless engine reopened the same project) go to the one focused last.
/// - Neither: the GUI engine focused most recently, if any. Else the only engine; several
///   headless engines are [`SelectError::Ambiguous`].
///
/// # Errors
/// See [`SelectError`].
pub fn select(
    mut engines: Vec<RunFileContents>,
    selector: &Selector,
) -> Result<RunFileContents, SelectError> {
    if engines.is_empty() {
        return Err(SelectError::NoEngine);
    }
    sort_by_focus(&mut engines);
    if let Some(pid) = selector.pid {
        engines.retain(|e| e.pid == pid);
    }
    if let Some(wanted) = &selector.project {
        engines.retain(|e| e.project.as_ref().is_some_and(|p| p.matches(wanted)));
    }
    if !selector.is_empty() {
        return engines.into_iter().next().ok_or(SelectError::NoMatch);
    }
    if let Some(pos) = engines.iter().position(|e| e.mode == RunMode::Gui) {
        return Ok(engines.swap_remove(pos));
    }
    match engines.len() {
        1 => Ok(engines.remove(0)),
        _ => Err(SelectError::Ambiguous),
    }
}

/// The run-file directory: `dir`, or the default `~/.cutweave/run`.
#[must_use]
pub fn run_dir(dir: Option<&Path>) -> Option<PathBuf> {
    dir.map(Path::to_path_buf).or_else(RunFile::default_dir)
}

/// What an engine's entry in `cutweave engines` and `engines_list` says. Never a token.
#[must_use]
pub fn describe(engine: &RunFileContents) -> Value {
    let mut out = json!({
        "pid": engine.pid,
        "mode": engine.mode.as_str(),
        "project": engine.project.as_ref().map(|p| json!({"name": p.name, "path": p.path})),
        "port": engine.port,
        "started": engine.started,
        "version": engine.version,
    });
    if let (Some(at), Value::Object(map)) = (engine.focused_at, &mut out) {
        map.insert("focused_at".into(), json!(at));
    }
    out
}

/// A list of engines as JSON, in the order given.
#[must_use]
pub fn describe_all(engines: &[RunFileContents]) -> Value {
    Value::Array(engines.iter().map(describe).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runfile::RunFileProject;

    fn engine(
        pid: u32,
        mode: RunMode,
        project: Option<&str>,
        focused: Option<u64>,
    ) -> RunFileContents {
        RunFileContents {
            pid,
            port: 1000 + u16::try_from(pid).unwrap(),
            agent_tokens: Vec::new(),
            project: project.map(|name| RunFileProject {
                name: name.into(),
                path: format!("/work/{name}"),
            }),
            mode,
            focused_at: focused,
            started: u64::from(pid),
            version: "0".into(),
            shm: None,
        }
    }

    fn sel(pid: Option<u32>, project: Option<&str>) -> Selector {
        Selector {
            pid,
            project: project.map(str::to_owned),
        }
    }

    #[test]
    fn a_project_is_chosen_by_name_in_any_case_or_by_path() {
        let all = vec![
            engine(1, RunMode::Headless, Some("Alpha"), None),
            engine(2, RunMode::Headless, Some("Beta"), None),
            engine(3, RunMode::Gui, Some("Gamma"), Some(5)),
        ];
        assert_eq!(
            select(all.clone(), &sel(None, Some("beta"))).unwrap().pid,
            2
        );
        assert_eq!(
            select(all.clone(), &sel(None, Some("/work/Alpha")))
                .unwrap()
                .pid,
            1
        );
        assert_eq!(
            select(all.clone(), &sel(None, Some("/work/Alpha/main.cutweave")))
                .unwrap()
                .pid,
            1
        );
        let no_match = |s: &Selector| select(all.clone(), s).unwrap_err();
        assert_eq!(no_match(&sel(None, Some("nope"))), SelectError::NoMatch);
        assert_eq!(select(all.clone(), &sel(Some(2), None)).unwrap().pid, 2);
        assert_eq!(no_match(&sel(Some(9), None)), SelectError::NoMatch);
    }

    #[test]
    fn with_no_selector_the_gui_engine_focused_last_wins() {
        let all = vec![
            engine(1, RunMode::Headless, Some("A"), None),
            engine(2, RunMode::Gui, Some("B"), Some(10)),
            engine(3, RunMode::Gui, Some("C"), Some(20)),
            engine(4, RunMode::Gui, None, None),
        ];
        assert_eq!(select(all, &Selector::default()).unwrap().pid, 3);
        // A GUI engine never focused is still preferred to a headless one.
        let all = vec![
            engine(1, RunMode::Headless, None, None),
            engine(4, RunMode::Gui, None, None),
        ];
        assert_eq!(select(all, &Selector::default()).unwrap().pid, 4);
    }

    #[test]
    fn headless_engines_are_ambiguous_unless_there_is_one() {
        let two = vec![
            engine(1, RunMode::Headless, Some("A"), None),
            engine(2, RunMode::Headless, Some("B"), None),
        ];
        assert_eq!(
            select(two.clone(), &Selector::default()).unwrap_err(),
            SelectError::Ambiguous
        );
        assert_eq!(
            select(two[..1].to_vec(), &Selector::default()).unwrap().pid,
            1
        );
        assert_eq!(
            select(Vec::new(), &Selector::default()).unwrap_err(),
            SelectError::NoEngine
        );
    }

    #[test]
    fn listing_is_newest_focus_first() {
        let mut all = vec![
            engine(1, RunMode::Headless, None, None),
            engine(2, RunMode::Gui, None, Some(10)),
            engine(3, RunMode::Gui, None, Some(20)),
            engine(5, RunMode::Headless, None, None),
        ];
        sort_by_focus(&mut all);
        let pids: Vec<u32> = all.iter().map(|e| e.pid).collect();
        assert_eq!(pids, [3, 2, 5, 1]);
    }

    #[test]
    fn a_description_has_no_secrets() {
        let mut e = engine(7, RunMode::Gui, Some("P"), Some(3));
        e.agent_tokens = vec![crate::runfile::RunFileToken {
            label: "cli".into(),
            token: "sekrit".into(),
        }];
        let text = describe(&e).to_string();
        assert!(!text.contains("sekrit") && !text.contains("agent_tokens"));
        assert!(text.contains("\"focused_at\":3") && text.contains("\"mode\":\"gui\""));
    }
}
