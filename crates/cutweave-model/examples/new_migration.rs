// SPDX-License-Identifier: GPL-3.0-or-later
//! Prints the scaffold for the next schema migration (WP-1A.03): the three things to add when
//! the project format changes. Run with `cargo run -p cutweave-model --example new_migration`.
//! It only prints; it never writes files.

use cutweave_model::CURRENT_SCHEMA_VERSION;

fn main() {
    let from = CURRENT_SCHEMA_VERSION;
    let to = from + 1;
    let module = format!("v{from}_to_v{to}");
    let ty = format!("V{from}ToV{to}");
    print!(
        r#"New schema migration v{from} -> v{to}. Add these 3 files (paths relative to crates/cutweave-model):

1. src/migrate/{module}.rs
--------------------------------------------------------------------------------------------
// SPDX-License-Identifier: GPL-3.0-or-later
//! v{from} -> v{to}: <one line: what changed in the format and why>.

use serde_json::Value;

use super::{{Migration, MigrationError}};

/// The v{from} -> v{to} migration.
pub(super) struct {ty};

impl Migration for {ty} {{
    fn source_version(&self) -> u32 {{
        {from}
    }}

    fn description(&self) -> &'static str {{
        "<what changed>"
    }}

    fn migrate(&self, doc: &mut Value, notes: &mut Vec<String>) -> Result<(), MigrationError> {{
        // Edit the raw JSON only; never call the current Rust types from a migration.
        // Report bad input with MigrationError::step({from}, "/json/pointer", "message").
        let _ = (doc, notes);
        Ok(())
    }}
}}
--------------------------------------------------------------------------------------------

2. src/migrate/mod.rs (registry)
   - add `mod {module};`
   - append `&{module}::{ty}` to MIGRATIONS
   - set `pub const CURRENT_SCHEMA_VERSION: u32 = {to};`
   - update the Rust types, then `tests/migrations.rs` (expected_* projects) to the new shape

3. tests/fixtures/v{to}/*.cutweave (golden fixtures of the new current version)
   - generate them from the expected projects:
     CUTWEAVE_BLESS_FIXTURES=1 cargo nextest run -p cutweave-model -E 'binary(migrations)'
   - never edit or regenerate tests/fixtures/v{from}/ and older: they must keep migrating to the
     expected projects (every_fixture_migrates_to_a_valid_current_project)
   - review the snapshot diff: cargo insta review
"#
    );
}
