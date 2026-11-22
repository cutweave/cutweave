//! Writes `schema/commands.json`, `schema/queries.json` and `ts/src/generated/commands.ts`
//! (relative to the spike root) from the registry. `--check` fails instead of writing if any
//! file is out of date (the CI form of the schema/codegen diff).

use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let artifacts = match s5_bus::codegen::artifacts(s5_ops::registry()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("s5-codegen: TypeScript export failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut stale = 0;
    for a in artifacts {
        let path = root.join(a.path);
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == a.contents {
            println!("up to date  {}", a.path);
            continue;
        }
        if check {
            eprintln!("STALE       {}", a.path);
            stale += 1;
            continue;
        }
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("s5-codegen: cannot create {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = std::fs::write(&path, &a.contents) {
            eprintln!("s5-codegen: cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote       {}", a.path);
    }
    if stale > 0 {
        eprintln!("s5-codegen: {stale} file(s) stale; run `cargo run -p s5-ops --bin s5-codegen`");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
