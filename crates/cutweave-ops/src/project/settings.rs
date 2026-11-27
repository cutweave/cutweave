// SPDX-License-Identifier: GPL-3.0-or-later
//! Project settings (WP-2H.08): `project.set_settings`, one undoable merge patch for every key
//! of `project.settings`, and the one code path behind it that `project.new{settings}` and the
//! named shortcuts (`media.set_proxy_policy`) share: [`patched`] checks a patch against the
//! `ProjectSettings` schema and then the values ([`check_format`], [`check_still`]).
//!
//! The patch type ([`SettingsPatch`]) derives its schema from `ProjectSettings`, so a key a
//! later work package adds to the struct is patchable, listed for agents and drawn by the
//! Project settings dialog without a change here. A key with a range adds its check to
//! [`check`].

use cutweave_commands::{CmdError, Command, QueryCtx, Registry, RegistryError, Tx, command, wire};
use cutweave_model::{
    Duration, Invariant, MAX_STILL_DURATION_SECS, Project, ProjectSettings, SequenceFormat,
    SettingsInEffect, SettingsPatch,
};

/// Registers `project.set_settings`.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry.register::<ProjectSetSettings>().map(drop)
}

/// The settings key `default_format`.
pub(crate) const DEFAULT_FORMAT: &str = "default_format";
/// The settings key `still_duration`.
pub(crate) const STILL_DURATION: &str = "still_duration";
/// The settings key `proxy_policy`.
pub(crate) const PROXY_POLICY: &str = "proxy_policy";

/// Why `format` cannot be a project's default format, if it cannot.
pub(crate) fn check_format(format: &SequenceFormat) -> Result<(), String> {
    let mut probe = Project::new("format check");
    probe.settings.default_format = format.clone();
    for v in probe.violations() {
        if let Invariant::InvalidFormat { reason, .. } = v {
            return Err(reason);
        }
    }
    Ok(())
}

/// Why `length` cannot be a project's still duration, if it cannot: it is positive and at
/// most one hour.
pub(crate) fn check_still(length: Duration) -> Result<(), String> {
    if length.is_zero() {
        return Err("the duration must be positive".to_owned());
    }
    let longest = Duration::from_secs(MAX_STILL_DURATION_SECS).map_err(|e| e.to_string())?;
    if length > longest {
        return Err("a still can be at most one hour long by default".to_owned());
    }
    Ok(())
}

/// The semantic checks of every key: `Err((key, reason))` for the first that fails.
pub(crate) fn check(settings: &ProjectSettings) -> Result<(), (&'static str, String)> {
    check_format(&settings.default_format).map_err(|reason| (DEFAULT_FORMAT, reason))?;
    if let Some(length) = settings.still_duration {
        check_still(length).map_err(|reason| (STILL_DURATION, reason))?;
    }
    Ok(())
}

/// `current` with `patch` merged in, checked against the schema (an unknown key or a value of
/// the wrong type fails with `Invalid` at its path under `at`, the pointer of the param that
/// holds the patch). The values are not checked: [`check`] does that.
pub(crate) fn merged(
    current: &ProjectSettings,
    patch: &SettingsPatch,
    at: &str,
) -> Result<ProjectSettings, CmdError> {
    current
        .patched(patch)
        .map_err(|e| CmdError::invalid(format!("{at}{}", e.pointer), e.reason))
}

/// [`merged`] and [`check`]: the settings a patch leads to, or why it is refused.
pub(crate) fn patched(
    current: &ProjectSettings,
    patch: &SettingsPatch,
    at: &str,
) -> Result<ProjectSettings, CmdError> {
    let next = merged(current, patch, at)?;
    check(&next).map_err(|(key, reason)| CmdError::invalid(format!("{at}/{key}"), reason))?;
    Ok(next)
}

/// Applies `patch` to the project's settings: the one way a command changes them.
pub(crate) fn apply_patch(
    tx: &mut Tx<'_>,
    patch: &SettingsPatch,
    at: &str,
) -> Result<ProjectSettings, CmdError> {
    let next = patched(&tx.doc().settings, patch, at)?;
    tx.update_settings(|s| {
        s.clone_from(&next);
        Ok(())
    })?;
    Ok(next)
}

/// Result of `project.set_settings`.
#[wire]
pub struct ProjectSettingsOutput {
    /// The project's settings now.
    pub settings: SettingsInEffect,
}

/// Changes project settings: `patch` holds the keys to change (those of `project.info`'s
/// `settings`), the rest keep their value. `null` puts an optional key back to its default;
/// an object value (`default_format`) is merged key by key. One undo step. Changing a setting
/// never alters what exists: sequences keep their format and clips their length.
#[command(
    name = "project.set_settings",
    summary = "Change project settings (a patch of project.info's settings)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectSetSettings {
    /// The settings to change, e.g. `{"proxy_policy": "off"}`. Times are flicks or seconds
    /// (`"3s"`).
    pub patch: SettingsPatch,
}

impl Command for ProjectSetSettings {
    type Output = ProjectSettingsOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        patched(&ctx.doc().settings, &self.patch, "/patch").map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ProjectSettingsOutput, CmdError> {
        let settings = apply_patch(tx, &self.patch, "/patch")?;
        Ok(ProjectSettingsOutput {
            settings: SettingsInEffect(settings),
        })
    }
}
