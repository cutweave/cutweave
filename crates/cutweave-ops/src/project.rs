// SPDX-License-Identifier: GPL-3.0-or-later
//! The `project` namespace (WP-1B.05; arch §5.1): `project.new`, `project.info`,
//! `project.rename`; the project's settings (WP-2H.08: [`settings`]); collecting a project's files into one folder (WP-2F.04: [`collect`]) and
//! archiving it as a folder or a zip (WP-2F.04b: [`archive`]).
//!
//! Opening, saving and closing a project need disk access and live in the engine
//! (`project.open`, WP-1I.04); these commands only edit the document they are given.

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    Duration, FrameRate, ProjectSettings, ProxyPolicy, SampleRate, Sequence, SequenceFormat,
    SequenceId, SettingsInEffect, SettingsPatch, TimeInput, Track, TrackId, TrackKind,
};

use crate::util::{check_name, resolve_duration};

pub mod archive;
pub mod collect;
pub mod settings;

/// Name of the sequence `project.new` creates.
pub const DEFAULT_SEQUENCE_NAME: &str = "Sequence 1";

/// Registers the `project.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ProjectNew>()?
        .register::<ProjectInfo>()?
        .register::<ProjectRename>()?;
    settings::register(registry)?;
    collect::register(registry)?;
    archive::register(registry)
}

/// The format of the first sequence and of new sequences. Absent fields keep the default
/// (1920x1080, 30 fps, 48 kHz).
#[wire]
#[derive(Default)]
pub struct FormatSpec {
    /// Canvas width in pixels, `1..=16384`.
    pub width: Option<u32>,
    /// Canvas height in pixels, `1..=16384`.
    pub height: Option<u32>,
    /// The edit grid, `1..=1000` fps and a whole number of flicks per frame.
    pub frame_rate: Option<FrameRate>,
    /// The mix rate in Hz, `8000..=384000`.
    pub sample_rate: Option<SampleRate>,
}

impl FormatSpec {
    fn resolve(&self) -> SequenceFormat {
        let mut f = SequenceFormat::default();
        f.width = self.width.unwrap_or(f.width);
        f.height = self.height.unwrap_or(f.height);
        f.frame_rate = self.frame_rate.unwrap_or(f.frame_rate);
        f.sample_rate = self.sample_rate.unwrap_or(f.sample_rate);
        f
    }
}

/// Result of `project.new`.
#[wire]
pub struct ProjectNewOutput {
    /// The created sequence.
    pub sequence: SequenceId,
    /// Its video track `V1`.
    pub video_track: TrackId,
    /// Its audio track `A1`.
    pub audio_track: TrackId,
}

/// Initializes an empty project: sets the project name and settings and creates one sequence
/// with a video track `V1` on top of an audio track `A1`. Fails with `Invalid` if the project
/// already has media or sequences, or if a setting is given both in `settings` and as its own
/// param.
#[command(
    name = "project.new",
    summary = "Initialize an empty project with one sequence (V1 + A1)",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = false)
)]
pub struct ProjectNew {
    /// Project name (1..=200 characters).
    pub name: String,
    /// Format of the first sequence; also the default for new sequences.
    pub format: Option<FormatSpec>,
    /// How long a still image is when it is placed without a length (up to one hour); absent:
    /// five seconds. The editor passes the person's "Still image duration" setting.
    pub still_duration: Option<TimeInput>,
    /// Which of the project's video gets proxies automatically (`off`, `auto`, `all`; ADR-0027);
    /// absent: `auto`. The editor passes the person's "Proxies for new projects" setting.
    pub proxy_policy: Option<ProxyPolicy>,
    /// Project settings to start with, as in `project.set_settings`; absent keys take their
    /// default. `default_format` here is the full form of `format`.
    pub settings: Option<SettingsPatch>,
}

impl ProjectNew {
    /// The settings the project starts with: the defaults, then `settings`, then the params
    /// that name one key (`format`, `still_duration`, `proxy_policy`). A key given both ways
    /// is refused.
    fn settings(&self) -> Result<ProjectSettings, CmdError> {
        let mut next = ProjectSettings::default();
        if let Some(patch) = &self.settings {
            for (key, param, given) in [
                (settings::DEFAULT_FORMAT, "format", self.format.is_some()),
                (
                    settings::STILL_DURATION,
                    "still_duration",
                    self.still_duration.is_some(),
                ),
                (
                    settings::PROXY_POLICY,
                    "proxy_policy",
                    self.proxy_policy.is_some(),
                ),
            ] {
                if given && patch.has(key) {
                    return Err(CmdError::invalid(
                        format!("/settings/{key}"),
                        format!("`{key}` is given twice: here and as `{param}`; give it once"),
                    ));
                }
            }
            next = settings::merged(&next, patch, "/settings")?;
        }
        if let Some(format) = &self.format {
            next.default_format = format.resolve();
        }
        let pointer = |own: bool, param: &str, key: &str| {
            if own {
                format!("/{param}")
            } else {
                format!("/settings/{key}")
            }
        };
        settings::check_format(&next.default_format).map_err(|reason| {
            CmdError::invalid(
                pointer(self.format.is_some(), "format", settings::DEFAULT_FORMAT),
                reason,
            )
        })?;
        if let Some(input) = self.still_duration {
            let rate = next.default_format.frame_rate;
            next.still_duration = Some(resolve_duration("/still_duration", input, rate)?);
        }
        if let Some(length) = next.still_duration {
            settings::check_still(length).map_err(|reason| {
                CmdError::invalid(
                    pointer(
                        self.still_duration.is_some(),
                        "still_duration",
                        settings::STILL_DURATION,
                    ),
                    reason,
                )
            })?;
        }
        if let Some(policy) = self.proxy_policy {
            next.proxy_policy = policy;
        }
        Ok(next)
    }
}

impl Command for ProjectNew {
    type Output = ProjectNewOutput;

    fn validate(&self, ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        check_name("/name", &self.name)?;
        if !ctx.doc().sequences.is_empty() || !ctx.doc().media.is_empty() {
            return Err(CmdError::invalid(
                "",
                "the project is not empty; project.new only initializes an empty project",
            ));
        }
        self.settings().map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ProjectNewOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let name = check_name("/name", &self.name)?;
        let settings = self.settings()?;
        let format: SequenceFormat = settings.default_format.clone();
        let mut sequence = Sequence::new(tx.new_id(), DEFAULT_SEQUENCE_NAME, format);
        let video = Track::new(tx.new_id(), TrackKind::Video, "V1");
        let audio = Track::new(tx.new_id(), TrackKind::Audio, "A1");
        let out = ProjectNewOutput {
            sequence: sequence.id,
            video_track: video.id,
            audio_track: audio.id,
        };
        sequence.insert_track(0, video);
        sequence.insert_track(1, audio);
        tx.update_meta(|m| {
            m.name = name;
            Ok(())
        })?;
        tx.update_settings(|s| {
            // Fields of a newer version that an empty document carries are kept.
            let extra = std::mem::take(&mut s.extra);
            *s = settings;
            s.extra = extra;
            Ok(())
        })?;
        tx.insert_sequence(&sequence)?;
        Ok(out)
    }
}

/// Summary of one sequence.
#[wire]
pub struct SequenceBrief {
    /// The sequence.
    pub id: SequenceId,
    /// Its name.
    pub name: String,
    /// Its format.
    pub format: SequenceFormat,
    /// End of the last item on any track.
    pub duration: Duration,
    /// Number of video tracks.
    pub video_tracks: u32,
    /// Number of audio tracks.
    pub audio_tracks: u32,
    /// Number of items on all its tracks.
    pub item_count: u32,
}

/// Result of `project.info`.
#[wire]
pub struct ProjectInfoOutput {
    /// Project name.
    pub name: String,
    /// Document schema version.
    pub schema_version: u32,
    /// Creation time (RFC 3339 UTC), if recorded.
    pub created: Option<String>,
    /// Format of new sequences.
    pub default_format: SequenceFormat,
    /// The project's settings (`project.set_settings` changes them).
    pub settings: SettingsInEffect,
    /// Number of media assets.
    pub media_count: u32,
    /// The sequences, ordered by ID.
    pub sequences: Vec<SequenceBrief>,
    /// The folders agents may name in a path right now (import media from, open projects in,
    /// save into): the folders the user allowed in Settings > Agents and MCP (or `--allow-root`)
    /// and the project folders. Filled by the engine; empty outside one.
    pub allowed_roots: Vec<String>,
    /// Undo steps kept per person or agent (`edit.undo` reaches no further back). The person
    /// sets it in Settings > Editing; 1000 until then. Filled by the engine.
    pub history_limit: u32,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The summary of one sequence (`project.info`, `sequence.list`).
pub(crate) fn sequence_brief(s: &Sequence) -> SequenceBrief {
    let of_kind = |k| s.tracks.values().filter(|t| t.kind == k).count();
    SequenceBrief {
        id: s.id,
        name: s.name.clone(),
        format: s.format.clone(),
        duration: s.duration(),
        video_tracks: count(of_kind(TrackKind::Video)),
        audio_tracks: count(of_kind(TrackKind::Audio)),
        item_count: count(s.tracks.values().map(|t| t.items.len()).sum()),
    }
}

/// Returns the project name, format, media count and a summary of each sequence.
#[query(
    name = "project.info",
    summary = "Get project name, default format and a summary of its sequences",
    mcp = "core"
)]
pub struct ProjectInfo {}

impl Query for ProjectInfo {
    type Output = ProjectInfoOutput;

    fn run(&self, ctx: &QueryCtx<'_>) -> Result<ProjectInfoOutput, CmdError> {
        let doc = ctx.doc();
        let sequences = doc.sequences.values().map(sequence_brief).collect();
        Ok(ProjectInfoOutput {
            name: doc.meta.name.clone(),
            schema_version: doc.meta.schema_version,
            created: doc.meta.created.clone(),
            default_format: doc.settings.default_format.clone(),
            settings: SettingsInEffect::from(&doc.settings),
            media_count: count(doc.media.len()),
            sequences,
            allowed_roots: Vec::new(),
            history_limit: u32::try_from(cutweave_commands::DEFAULT_UNDO_DEPTH).unwrap_or(u32::MAX),
        })
    }
}

/// Result of `project.rename`.
#[wire]
pub struct ProjectRenameOutput {
    /// The name now in effect (trimmed).
    pub name: String,
}

/// Renames the project.
#[command(
    name = "project.rename",
    summary = "Rename the project",
    undoable,
    mcp = "catalog",
    annotations(destructive = false, idempotent = true)
)]
pub struct ProjectRename {
    /// New project name (1..=200 characters).
    pub name: String,
}

impl Command for ProjectRename {
    type Output = ProjectRenameOutput;

    fn validate(&self, _ctx: &QueryCtx<'_>) -> Result<(), CmdError> {
        check_name("/name", &self.name).map(drop)
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ProjectRenameOutput, CmdError> {
        let name = check_name("/name", &self.name)?;
        tx.update_meta(|m| {
            m.name.clone_from(&name);
            Ok(())
        })?;
        Ok(ProjectRenameOutput { name })
    }
}
