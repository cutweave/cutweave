// SPDX-License-Identifier: GPL-3.0-or-later
//! The `project` namespace (WP-1B.05; arch §5.1): `project.new`, `project.info`,
//! `project.rename`; collecting a project's files into one folder (WP-2F.04: [`collect`]) and
//! archiving it as a folder or a zip (WP-2F.04b: [`archive`]).
//!
//! Opening, saving and closing a project need disk access and live in the engine
//! (`project.open`, WP-1I.04); these commands only edit the document they are given.

use cutweave_commands::{
    CmdError, Command, Query, QueryCtx, Registry, RegistryError, Tx, command, query, wire,
};
use cutweave_model::{
    Duration, FrameRate, Invariant, MAX_STILL_DURATION_SECS, Project, ProxyPolicy, SampleRate,
    Sequence, SequenceFormat, SequenceId, TimeInput, Track, TrackId, TrackKind,
};

use crate::util::{check_name, resolve_duration};

pub mod archive;
pub mod collect;

/// Name of the sequence `project.new` creates.
pub const DEFAULT_SEQUENCE_NAME: &str = "Sequence 1";

/// Registers the `project.*` commands and queries.
pub fn register(registry: &mut Registry) -> Result<(), RegistryError> {
    registry
        .register::<ProjectNew>()?
        .register::<ProjectInfo>()?
        .register::<ProjectRename>()?;
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

/// Initializes an empty project: sets the project name and default format and creates one
/// sequence with a video track `V1` on top of an audio track `A1`. Fails with `Invalid` if the
/// project already has media or sequences.
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
}

impl ProjectNew {
    /// The still duration to store, if one was given.
    fn still(&self, format: &SequenceFormat) -> Result<Option<Duration>, CmdError> {
        let Some(input) = self.still_duration else {
            return Ok(None);
        };
        let length = resolve_duration("/still_duration", input, format.frame_rate)?;
        let longest = Duration::from_secs(MAX_STILL_DURATION_SECS)
            .map_err(|e| CmdError::internal(e.to_string()))?;
        if length > longest {
            return Err(CmdError::invalid(
                "/still_duration",
                "a still can be at most one hour long by default",
            ));
        }
        Ok(Some(length))
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
        let format = self.format.clone().unwrap_or_default().resolve();
        self.still(&format)?;
        let mut probe = Project::new("format check");
        probe.settings.default_format = format;
        for v in probe.violations() {
            if let Invariant::InvalidFormat { reason, .. } = v {
                return Err(CmdError::invalid("/format", reason));
            }
        }
        Ok(())
    }

    fn apply(&self, tx: &mut Tx<'_>) -> Result<ProjectNewOutput, CmdError> {
        self.validate(&tx.query_ctx())?;
        let name = check_name("/name", &self.name)?;
        let format = self.format.clone().unwrap_or_default().resolve();
        let still = self.still(&format)?;
        let mut sequence = Sequence::new(tx.new_id(), DEFAULT_SEQUENCE_NAME, format.clone());
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
            s.default_format = format;
            s.still_duration = still;
            s.proxy_policy = self.proxy_policy.unwrap_or_default();
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
    /// Number of media assets.
    pub media_count: u32,
    /// The sequences, ordered by ID.
    pub sequences: Vec<SequenceBrief>,
    /// The folders agents may name in a path right now (import media from, open projects in,
    /// save into): the folders the user allowed in Settings > Agents and MCP (or `--allow-root`)
    /// and the project folders. Filled by the engine; empty outside one.
    pub allowed_roots: Vec<String>,
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
            media_count: count(doc.media.len()),
            sequences,
            allowed_roots: Vec::new(),
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
