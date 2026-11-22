// SPDX-License-Identifier: GPL-3.0-or-later
//! Schema versioning and migrations (arch §4.3, ADR-0006).
//!
//! Every document carries `meta.schema_version`. Opening one:
//!
//! - **older** than [`CURRENT_SCHEMA_VERSION`]: the registered [`Migration`]s run in order
//!   (`vN → vN+1`) on the raw JSON, then the result is parsed as a [`Project`];
//! - **current**: parsed directly;
//! - **newer** (saved by a later Cutweave): parsed best-effort and returned as
//!   [`OpenOutcome::ReadOnly`]; fields this version does not know are kept in `extra`, and the
//!   document must never be saved over.
//!
//! Adding a version: write `src/migrate/v<N>_to_v<N+1>.rs`, append it to [`MIGRATIONS`], bump
//! [`CURRENT_SCHEMA_VERSION`] and add golden fixtures under `tests/fixtures/v<N+1>/`
//! (`cargo run -p cutweave-model --example new_migration` prints the scaffold). A test fails
//! if a version has no fixture directory.
//!
//! ```
//! use cutweave_model::{OpenOutcome, Project, open_str, to_document_string};
//!
//! let saved = to_document_string(&Project::new("Demo"))?;
//! let OpenOutcome::Editable { project, report } = open_str(&saved)? else {
//!     unreachable!("a current document is editable");
//! };
//! assert_eq!(project.meta.name, "Demo");
//! assert!(!report.migrated());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod v0_to_v1;

use std::fmt;

use serde_json::{Map, Value};

use crate::Project;

/// The schema version this build reads, edits and writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The oldest schema version that can still be migrated (v0 is the pre-release draft format).
pub const OLDEST_SCHEMA_VERSION: u32 = 0;

/// The migration chain, ordered: entry `i` reads version `OLDEST_SCHEMA_VERSION + i`.
pub static MIGRATIONS: &[&dyn Migration] = &[&v0_to_v1::V0ToV1];

/// One step of the chain: rewrites a raw JSON document from [`Migration::source_version`] to the
/// next version.
///
/// Migrations work on `serde_json::Value` so they never depend on the current Rust types, which
/// keep changing after the migration is written.
pub trait Migration: Sync {
    /// The version this migration reads; it produces `source_version() + 1`.
    fn source_version(&self) -> u32;

    /// One line for reports and logs ("track items keyed by id").
    fn description(&self) -> &'static str;

    /// Rewrites `doc` in place. The runner updates `meta.schema_version` afterwards, so the
    /// migration must not. Messages pushed to `notes` end up in the [`MigrationReport`].
    fn migrate(&self, doc: &mut Value, notes: &mut Vec<String>) -> Result<(), MigrationError>;
}

/// Why a document could not be opened or migrated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MigrationError {
    /// The text is not JSON.
    #[error("the project document is not valid JSON: {0}")]
    Json(String),
    /// The JSON root is not an object.
    #[error("the project document must be a JSON object")]
    NotAnObject,
    /// No `meta.schema_version`.
    #[error("the project document has no meta.schema_version; is it a Cutweave project?")]
    MissingVersion,
    /// `meta.schema_version` is not a non-negative integer.
    #[error("meta.schema_version must be a non-negative integer, got {0}")]
    InvalidVersion(String),
    /// Older than the oldest version that can be migrated.
    #[error("schema version {found} is too old; the oldest supported is {oldest}")]
    TooOld {
        /// The document's version.
        found: u32,
        /// [`OLDEST_SCHEMA_VERSION`].
        oldest: u32,
    },
    /// Newer than this build; open it with [`open`] to get a read-only view.
    #[error("schema version {found} is newer than this version of Cutweave supports ({supported})")]
    NewerVersion {
        /// The document's version.
        found: u32,
        /// [`CURRENT_SCHEMA_VERSION`].
        supported: u32,
    },
    /// The registry has no migration out of `version` (a bug in the chain).
    #[error("no migration is registered from schema version {version}")]
    MissingMigration {
        /// The version without a migration.
        version: u32,
    },
    /// A migration step rejected the document.
    #[error("migrating from schema version {from} failed at `{path}`: {message}")]
    Step {
        /// The version the step reads.
        from: u32,
        /// JSON Pointer of the offending value.
        path: String,
        /// What is wrong.
        message: String,
    },
    /// The (migrated) document does not match the project format.
    #[error("the schema v{version} document is malformed at `{path}`: {message}")]
    Malformed {
        /// The version the document was parsed as.
        version: u32,
        /// Where (`sequences.seq_….tracks…`), as far as the parser knows.
        path: String,
        /// What is wrong.
        message: String,
    },
}

impl MigrationError {
    /// A [`MigrationError::Step`] for use inside migrations.
    #[must_use]
    pub fn step(from: u32, path: impl Into<String>, message: impl fmt::Display) -> Self {
        MigrationError::Step {
            from,
            path: path.into(),
            message: message.to_string(),
        }
    }
}

/// One applied migration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationStep {
    /// The version read.
    pub from_version: u32,
    /// The version written.
    pub to_version: u32,
    /// What the migration does.
    pub description: &'static str,
    /// Anything the migration wants the user or the log to know.
    pub notes: Vec<String>,
}

/// What opening a document did to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    /// The version the document was saved with.
    pub from_version: u32,
    /// The version it is now.
    pub to_version: u32,
    /// Applied migrations, in order.
    pub steps: Vec<MigrationStep>,
}

impl MigrationReport {
    /// Whether any migration ran (the document should be saved in the new format, and the
    /// original kept as a backup).
    #[must_use]
    pub fn migrated(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// The result of [`open`].
#[derive(Clone, Debug, PartialEq)]
pub enum OpenOutcome {
    /// At the current version (after any migrations): edit and save normally.
    Editable {
        /// The document.
        project: Project,
        /// What was migrated.
        report: MigrationReport,
    },
    /// Saved by a newer Cutweave: show it, but never save over it (arch §4.3).
    ReadOnly {
        /// The document as far as this version understands it; unknown fields are in `extra`.
        project: Project,
        /// The document's `meta.schema_version`.
        found_version: u32,
        /// Why it is read-only, for the warning shown to the user.
        reason: String,
    },
}

/// Reads `meta.schema_version`.
pub fn schema_version_of(doc: &Value) -> Result<u32, MigrationError> {
    let root = doc.as_object().ok_or(MigrationError::NotAnObject)?;
    let version = root
        .get("meta")
        .and_then(|meta| meta.get("schema_version"))
        .ok_or(MigrationError::MissingVersion)?;
    version
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| MigrationError::InvalidVersion(version.to_string()))
}

fn set_schema_version(doc: &mut Value, version: u32) -> Result<(), MigrationError> {
    let meta = doc
        .as_object_mut()
        .and_then(|root| root.get_mut("meta"))
        .and_then(Value::as_object_mut)
        .ok_or(MigrationError::MissingVersion)?;
    meta.insert("schema_version".into(), version.into());
    Ok(())
}

/// Runs a migration chain. [`migrate_value`] uses the registered chain; tests build their own.
pub struct Migrator<'a> {
    chain: &'a [&'a dyn Migration],
    oldest: u32,
    current: u32,
}

impl<'a> Migrator<'a> {
    /// A runner for `chain`, which must be contiguous and end at `current`: entry `i` reads
    /// `current - chain.len() + i`.
    pub fn new(chain: &'a [&'a dyn Migration], current: u32) -> Result<Self, MigrationError> {
        let len = u32::try_from(chain.len())
            .map_err(|_| MigrationError::MissingMigration { version: current })?;
        let oldest = current
            .checked_sub(len)
            .ok_or(MigrationError::MissingMigration { version: 0 })?;
        for (version, migration) in (oldest..).zip(chain) {
            if migration.source_version() != version {
                return Err(MigrationError::MissingMigration { version });
            }
        }
        Ok(Migrator {
            chain,
            oldest,
            current,
        })
    }

    /// The oldest version this chain reads.
    #[must_use]
    pub fn oldest(&self) -> u32 {
        self.oldest
    }

    /// Migrates `doc` to the chain's current version.
    pub fn migrate(&self, mut doc: Value) -> Result<(Value, MigrationReport), MigrationError> {
        let from_version = schema_version_of(&doc)?;
        if from_version > self.current {
            return Err(MigrationError::NewerVersion {
                found: from_version,
                supported: self.current,
            });
        }
        if from_version < self.oldest {
            return Err(MigrationError::TooOld {
                found: from_version,
                oldest: self.oldest,
            });
        }
        let mut steps = Vec::new();
        for migration in self
            .chain
            .iter()
            .skip_while(|m| m.source_version() < from_version)
        {
            let from = migration.source_version();
            let mut notes = Vec::new();
            migration.migrate(&mut doc, &mut notes)?;
            set_schema_version(&mut doc, from + 1)?;
            steps.push(MigrationStep {
                from_version: from,
                to_version: from + 1,
                description: migration.description(),
                notes,
            });
        }
        Ok((
            doc,
            MigrationReport {
                from_version,
                to_version: self.current,
                steps,
            },
        ))
    }
}

/// Migrates a raw document to [`CURRENT_SCHEMA_VERSION`] with the registered chain, without
/// parsing it (useful for diffs and snapshots of what migrations produce).
pub fn migrate_value(doc: Value) -> Result<(Value, MigrationReport), MigrationError> {
    Migrator::new(MIGRATIONS, CURRENT_SCHEMA_VERSION)?.migrate(doc)
}

fn parse_project(doc: Value, version: u32) -> Result<Project, MigrationError> {
    serde_path_to_error::deserialize(doc).map_err(|e| MigrationError::Malformed {
        version,
        path: e.path().to_string(),
        message: e.into_inner().to_string(),
    })
}

/// Migrates `doc` to [`CURRENT_SCHEMA_VERSION`] and parses it. A newer document is an error
/// here; [`open`] turns it into a read-only view instead.
///
/// The result is not validated: call [`Project::validate`] to check invariants.
pub fn migrate_to_current(doc: Value) -> Result<(Project, MigrationReport), MigrationError> {
    let (doc, report) = migrate_value(doc)?;
    Ok((parse_project(doc, CURRENT_SCHEMA_VERSION)?, report))
}

/// Opens a project document: migrates older versions, parses current ones and returns newer
/// ones read-only.
pub fn open(doc: Value) -> Result<OpenOutcome, MigrationError> {
    let found_version = schema_version_of(&doc)?;
    if found_version > CURRENT_SCHEMA_VERSION {
        let project = parse_project(doc, found_version)?;
        return Ok(OpenOutcome::ReadOnly {
            project,
            found_version,
            reason: format!(
                "This project was saved by a newer version of Cutweave (format v{found_version}; \
                 this version supports up to v{CURRENT_SCHEMA_VERSION}). It is open read-only so \
                 nothing is lost; update Cutweave to edit it."
            ),
        });
    }
    let (project, report) = migrate_to_current(doc)?;
    Ok(OpenOutcome::Editable { project, report })
}

/// [`open`] for the text of a `.cutweave` file.
pub fn open_str(text: &str) -> Result<OpenOutcome, MigrationError> {
    let doc = serde_json::from_str(text).map_err(|e| MigrationError::Json(e.to_string()))?;
    open(doc)
}

/// The text of a `.cutweave` file: pretty-printed JSON (two-space indent, fields in declaration
/// order, maps sorted by key) with a trailing newline. Byte-stable for equal projects.
pub fn to_document_string(project: &Project) -> Result<String, serde_json::Error> {
    let mut text = serde_json::to_string_pretty(project)?;
    text.push('\n');
    Ok(text)
}

/// The object at `doc[key]`, for migrations; `Ok(None)` if absent.
pub(crate) fn object_field<'v>(
    from: u32,
    parent: &'v mut Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<&'v mut Map<String, Value>>, MigrationError> {
    match parent.get_mut(key) {
        None => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err(MigrationError::step(
            from,
            format!("{path}/{key}"),
            "expected an object",
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Fake(u32);

    impl Migration for Fake {
        fn source_version(&self) -> u32 {
            self.0
        }

        fn description(&self) -> &'static str {
            "appends the version to `trail`"
        }

        fn migrate(&self, doc: &mut Value, notes: &mut Vec<String>) -> Result<(), MigrationError> {
            let trail = doc
                .as_object_mut()
                .and_then(|root| root.entry("trail").or_insert(json!([])).as_array_mut())
                .ok_or_else(|| MigrationError::step(self.0, "/trail", "not an array"))?;
            trail.push(self.0.into());
            notes.push(format!("ran {}", self.0));
            Ok(())
        }
    }

    fn doc(version: u32) -> Value {
        json!({ "meta": { "schema_version": version } })
    }

    #[test]
    fn the_registered_chain_is_contiguous_and_ends_at_current() {
        let migrator = Migrator::new(MIGRATIONS, CURRENT_SCHEMA_VERSION).unwrap();
        assert_eq!(migrator.oldest(), OLDEST_SCHEMA_VERSION);
        for (i, m) in MIGRATIONS.iter().enumerate() {
            assert_eq!(
                m.source_version(),
                OLDEST_SCHEMA_VERSION + u32::try_from(i).unwrap()
            );
            assert!(m.description().len() > 10, "{}", m.description());
        }
    }

    #[test]
    fn chains_run_in_order_from_the_document_version() {
        let chain: [&dyn Migration; 3] = [&Fake(2), &Fake(3), &Fake(4)];
        let migrator = Migrator::new(&chain, 5).unwrap();
        let (out, report) = migrator.migrate(doc(3)).unwrap();
        assert_eq!(out["trail"], json!([3, 4]));
        assert_eq!(out["meta"]["schema_version"], 5);
        assert_eq!((report.from_version, report.to_version), (3, 5));
        assert_eq!(report.steps.len(), 2);
        assert_eq!(report.steps[0].notes, ["ran 3"]);
        assert!(report.migrated());
        let (out, report) = migrator.migrate(doc(5)).unwrap();
        assert!(out.get("trail").is_none());
        assert!(!report.migrated());
        assert_eq!(
            migrator.migrate(doc(1)).unwrap_err(),
            MigrationError::TooOld {
                found: 1,
                oldest: 2
            }
        );
        assert_eq!(
            migrator.migrate(doc(6)).unwrap_err(),
            MigrationError::NewerVersion {
                found: 6,
                supported: 5
            }
        );
    }

    #[test]
    fn broken_chains_are_rejected() {
        let gap: [&dyn Migration; 2] = [&Fake(2), &Fake(4)];
        assert_eq!(
            Migrator::new(&gap, 5).err(),
            Some(MigrationError::MissingMigration { version: 3 })
        );
        let wrong_end: [&dyn Migration; 1] = [&Fake(2)];
        assert!(Migrator::new(&wrong_end, 5).is_err());
        let too_long: [&dyn Migration; 3] = [&Fake(0), &Fake(1), &Fake(2)];
        assert!(Migrator::new(&too_long, 2).is_err());
        let empty: [&dyn Migration; 0] = [];
        assert_eq!(Migrator::new(&empty, 7).unwrap().oldest(), 7);
    }

    #[test]
    fn version_detection() {
        assert_eq!(schema_version_of(&doc(4)), Ok(4));
        assert_eq!(
            schema_version_of(&json!([])),
            Err(MigrationError::NotAnObject)
        );
        assert_eq!(
            schema_version_of(&json!({})),
            Err(MigrationError::MissingVersion)
        );
        assert_eq!(
            schema_version_of(&json!({"meta": {}})),
            Err(MigrationError::MissingVersion)
        );
        for bad in [json!(-1), json!("1"), json!(1.5), json!(u64::MAX)] {
            let d = json!({ "meta": { "schema_version": bad } });
            assert!(matches!(
                schema_version_of(&d),
                Err(MigrationError::InvalidVersion(_))
            ));
        }
    }

    #[test]
    fn malformed_documents_report_a_path() {
        let mut value = serde_json::to_value(Project::new("x")).unwrap();
        value["settings"]["default_format"]["width"] = json!("wide");
        let err = migrate_to_current(value).unwrap_err();
        let MigrationError::Malformed { path, .. } = &err else {
            panic!("{err:?}");
        };
        assert_eq!(path, "settings.default_format.width");
        assert!(matches!(
            open_str("{ not json"),
            Err(MigrationError::Json(_))
        ));
    }
}
