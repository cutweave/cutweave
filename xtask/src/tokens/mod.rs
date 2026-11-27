// SPDX-License-Identifier: GPL-3.0-or-later
//! `cargo xtask tokens [--check]`: design token pipeline and contrast gate (WP-0K.01).
//!
//! Reads `design/tokens.json` and writes the CSS, Tailwind `@theme`, TypeScript and Rust
//! outputs. With `--check` nothing is written: stale outputs or failing contrast pairs fail.

mod contrast;
mod emit;
mod model;

use std::fs;
use std::path::{Path, PathBuf};

use model::Tokens;

/// Source of truth, relative to the workspace root.
const SOURCE: &str = "design/tokens.json";

/// A generated file: workspace-relative path and exact contents.
pub struct Output {
    pub path: &'static str,
    pub content: String,
}

/// Generates every output from the token JSON text.
pub fn generate(json: &str) -> Result<(Tokens, Vec<Output>), String> {
    let t = Tokens::parse(json)?;
    let css = emit::tokens_css(&t);
    let outputs = vec![
        Output {
            path: "packages/ui/src/tokens/tokens.css",
            content: css.clone(),
        },
        Output {
            path: "design/tokens.css",
            content: css,
        },
        Output {
            path: "packages/ui/src/tokens/theme.css",
            content: emit::theme_css(&t),
        },
        Output {
            path: "packages/tokens/src/generated/tokens.ts",
            content: emit::tokens_ts(&t)?,
        },
        Output {
            path: "crates/cutweave-tokens/src/generated.rs",
            content: emit::tokens_rs(&t)?,
        },
    ];
    Ok((t, outputs))
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

/// Paths of outputs whose on-disk contents differ from `outputs`.
fn stale(root: &Path, outputs: &[Output]) -> Vec<&'static str> {
    outputs
        .iter()
        .filter(|o| {
            fs::read_to_string(root.join(o.path)).map_or(true, |c| normalize(&c) != o.content)
        })
        .map(|o| o.path)
        .collect()
}

/// Runs the subcommand; returns true on success.
pub fn run(root: &Path, args: &[String]) -> bool {
    let mut check = false;
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            other => {
                eprintln!("tokens: unknown argument `{other}` (expected --check)");
                return false;
            }
        }
    }
    match run_inner(root, check) {
        Ok(ok) => ok,
        Err(e) => {
            eprintln!("tokens: {e}");
            false
        }
    }
}

fn run_inner(root: &Path, check: bool) -> Result<bool, String> {
    let src: PathBuf = root.join(SOURCE);
    let json =
        fs::read_to_string(&src).map_err(|e| format!("cannot read {}: {e}", src.display()))?;
    let (tokens, outputs) = generate(&json)?;

    let results = contrast::check(&tokens)?;
    let failures: Vec<_> = results.iter().filter(|r| !r.passes()).collect();
    print!("{}", contrast::table(&results));
    println!(
        "tokens: contrast {} pairs checked, {} failing",
        results.len(),
        failures.len()
    );

    // A density only makes sizes larger (model.rs checks it), so the pairs above hold in each.
    for d in &tokens.densities {
        println!(
            "tokens: density `{}` makes {} size and spacing tokens larger and changes no color",
            d.id,
            d.tokens.len()
        );
    }

    if check {
        let stale = stale(root, &outputs);
        for p in &stale {
            eprintln!("tokens: {p} is stale; run `cargo xtask tokens`");
        }
        if !failures.is_empty() {
            eprintln!("tokens: contrast gate failed");
        }
        let ok = stale.is_empty() && failures.is_empty();
        if ok {
            println!("tokens: ok ({} outputs up to date)", outputs.len());
        }
        return Ok(ok);
    }

    for o in &outputs {
        let path = root.join(o.path);
        if fs::read_to_string(&path).is_ok_and(|c| normalize(&c) == o.content) {
            continue;
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        fs::write(&path, &o.content).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("tokens: wrote {}", o.path);
    }
    if !failures.is_empty() {
        eprintln!("tokens: outputs written, but the contrast gate failed");
    }
    Ok(failures.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf)
    }

    fn real() -> (Tokens, Vec<Output>) {
        let json = fs::read_to_string(root().join(SOURCE)).unwrap();
        generate(&json).unwrap()
    }

    #[test]
    fn committed_outputs_match_generator() {
        let (_, outputs) = real();
        assert_eq!(stale(&root(), &outputs), Vec::<&str>::new());
    }

    #[test]
    fn generation_is_byte_identical_across_runs() {
        let (_, a) = real();
        let (_, b) = real();
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.content, y.content, "{}", x.path);
        }
    }

    #[test]
    fn changing_a_value_changes_every_output() {
        let json = fs::read_to_string(root().join(SOURCE)).unwrap();
        let (_, base) = generate(&json).unwrap();
        let changed = json.replacen("#f2b33d", "#f2b33e", 1);
        assert_ne!(json, changed);
        let (_, next) = generate(&changed).unwrap();
        for (x, y) in base.iter().zip(&next) {
            if x.path == "packages/ui/src/tokens/theme.css" {
                // theme.css only references variable names, never values.
                assert_eq!(x.content, y.content);
            } else {
                assert_ne!(x.content, y.content, "{}", x.path);
            }
        }
    }

    #[test]
    fn real_tokens_pass_the_contrast_gate() {
        let (t, _) = real();
        let results = contrast::check(&t).unwrap();
        let failing: Vec<_> = results
            .iter()
            .filter(|r| !r.passes())
            .map(|r| format!("{} {} on {}: {:.2}", r.theme, r.fg, r.ground, r.ratio))
            .collect();
        assert!(failing.is_empty(), "{failing:?}");
    }
}
