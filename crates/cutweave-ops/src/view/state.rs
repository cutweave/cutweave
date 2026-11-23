// SPDX-License-Identifier: GPL-3.0-or-later
//! The view state types and [`ViewCtx`], the context `view.*` commands run against.
//!
//! View state is **not** part of the document: it is not undoable, not journaled, has no `rev`
//! and emits no `doc.patched` (arch §5.1). It lives in its own small store so the document
//! actor can host it later (`ViewCtx` is the [`EngineCommand`](cutweave_commands::EngineCommand)
//! context), and the engine persists it to `.cutweave/ui-state.json`
//! (`cutweave_engine::ui_state`). This module is pure (no I/O, no clocks).

use cutweave_commands::{CmdError, wire};
use cutweave_model::{EntityKind, ItemId, MediaId, Project, SequenceId, Time, TimeInput, TrackId};
use serde_json::{Map, Value};

/// Version written to `ui-state.json`; files without it count as version 0 (still readable).
pub const UI_STATE_VERSION: u32 = 1;
/// Smallest accepted zoom (pixels per second of timeline).
pub const MIN_ZOOM: f64 = 0.001;
/// Largest accepted zoom (pixels per second of timeline).
pub const MAX_ZOOM: f64 = 100_000.0;
/// Largest accepted master gain (linear; 4.0 is about +12 dB).
pub const MAX_MASTER_GAIN: f64 = 4.0;
/// Most selected entities.
pub const MAX_SELECTION: usize = 10_000;
/// Largest accepted `panel_layout`, as serialized JSON.
pub const MAX_PANEL_LAYOUT_BYTES: usize = 256 * 1024;

/// Keys this version understands; any other key of the file is preserved verbatim.
const KNOWN_KEYS: &[&str] = &[
    "version",
    "playhead",
    "zoom",
    "scroll",
    "selection",
    "panel_layout",
    "preview_quality",
    "master_gain",
];

/// Preview resolution (arch §8 quality ladder).
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PreviewQuality {
    /// Full resolution.
    #[default]
    Full,
    /// Half resolution.
    Half,
    /// Quarter resolution.
    Quarter,
}

/// Timeline scroll offset in pixels, from the top-left of the timeline.
#[wire]
#[derive(Copy, Default)]
pub struct Scroll {
    /// Horizontal offset, `>= 0`.
    pub x: f64,
    /// Vertical offset, `>= 0`.
    pub y: f64,
}

/// One selected entity, by ID (never by index).
#[wire]
#[derive(Copy, Eq, Hash)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionRef {
    /// A timeline item.
    Item {
        /// The item.
        id: ItemId,
    },
    /// A track.
    Track {
        /// The track.
        id: TrackId,
    },
    /// A media asset.
    Media {
        /// The asset.
        id: MediaId,
    },
    /// A sequence.
    Sequence {
        /// The sequence.
        id: SequenceId,
    },
}

impl SelectionRef {
    /// `Ok` if the entity exists in `doc`, else `NotFound`.
    pub fn check_exists(self, doc: &Project) -> Result<(), CmdError> {
        let found = match self {
            SelectionRef::Item { id } => doc.find_item(&id).is_some(),
            SelectionRef::Track { id } => doc.sequences.values().any(|s| s.track(&id).is_some()),
            SelectionRef::Media { id } => doc.media.contains_key(&id),
            SelectionRef::Sequence { id } => doc.sequences.contains_key(&id),
        };
        if found {
            return Ok(());
        }
        Err(match self {
            SelectionRef::Item { id } => CmdError::not_found(EntityKind::Item, id),
            SelectionRef::Track { id } => CmdError::not_found(EntityKind::Track, id),
            SelectionRef::Media { id } => CmdError::not_found(EntityKind::Media, id),
            SelectionRef::Sequence { id } => CmdError::not_found(EntityKind::Sequence, id),
        })
    }
}

/// The UI view state of a project (arch §5.1). Persisted in `.cutweave/ui-state.json`.
#[wire]
pub struct UiState {
    /// Playhead position on the timeline, `>= 0`.
    pub playhead: Time,
    /// Timeline zoom in pixels per second.
    pub zoom: f64,
    /// Timeline scroll offset.
    pub scroll: Scroll,
    /// Selected entities, in selection order, without duplicates. Every ID existed when it was
    /// selected; IDs that no longer exist are dropped.
    pub selection: Vec<SelectionRef>,
    /// Dock/panel layout. Opaque to the engine (owned by the UI); `null` until the UI sets it.
    #[specta(type = specta_typescript::Unknown)]
    pub panel_layout: Value,
    /// Preview resolution.
    pub preview_quality: PreviewQuality,
    /// Master output gain (linear, `0..=4`, 1 = unity).
    pub master_gain: f64,
}

impl Default for UiState {
    fn default() -> UiState {
        UiState::initial()
    }
}

impl UiState {
    /// The defaults of a project that never had view state.
    #[must_use]
    pub fn initial() -> UiState {
        UiState {
            playhead: Time::ZERO,
            zoom: 100.0,
            scroll: Scroll::default(),
            selection: Vec::new(),
            panel_layout: Value::Null,
            preview_quality: PreviewQuality::Full,
            master_gain: 1.0,
        }
    }
}

/// A partial update of [`UiState`]: fields that are present replace the current value, absent
/// fields are left alone (JSON `null` counts as absent, so `panel_layout` is reset with `{}`).
#[wire]
#[derive(Default)]
pub struct ViewPatch {
    /// New playhead (flicks, seconds text; `>= 0`).
    #[serde(default)]
    pub playhead: Option<TimeInput>,
    /// New zoom in pixels per second.
    #[serde(default)]
    pub zoom: Option<f64>,
    /// New scroll offset.
    #[serde(default)]
    pub scroll: Option<Scroll>,
    /// New selection (replaces the whole selection; every ID must exist).
    #[serde(default)]
    pub selection: Option<Vec<SelectionRef>>,
    /// New panel layout (opaque JSON, replaces the whole value).
    #[serde(default)]
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub panel_layout: Option<Value>,
    /// New preview resolution.
    #[serde(default)]
    pub preview_quality: Option<PreviewQuality>,
    /// New master gain (linear, `0..=4`).
    #[serde(default)]
    pub master_gain: Option<f64>,
}

/// How `view.select` combines its IDs with the current selection.
#[wire]
#[derive(Copy, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SelectMode {
    /// The IDs become the selection.
    #[default]
    Replace,
    /// The IDs are appended (already selected ones keep their place).
    Add,
    /// The IDs are removed from the selection.
    Remove,
    /// Selected IDs are removed, the others appended.
    Toggle,
}

/// The `view.changed` event payload, sent to every client except the one that made the change.
#[wire]
pub struct ViewChanged {
    /// Counts changes since the project was opened (wraps never in practice; saturates).
    #[specta(type = specta_typescript::Number)]
    pub view_rev: u32,
    /// The state after the change.
    pub state: UiState,
}

/// What `view.*` commands run against: the state, the unknown keys of the file, and the
/// document snapshot selections are validated against.
#[derive(Clone, Debug)]
pub struct ViewCtx {
    state: UiState,
    extra: Map<String, Value>,
    doc: Project,
    rev: u32,
    event_pending: bool,
}

pub(crate) fn check_time(t: Time) -> Result<Time, String> {
    if t < Time::ZERO {
        Err("the playhead cannot be negative".into())
    } else {
        Ok(t)
    }
}

pub(crate) fn check_zoom(z: f64) -> Result<f64, String> {
    if z.is_finite() && (MIN_ZOOM..=MAX_ZOOM).contains(&z) {
        Ok(z)
    } else {
        Err(format!("zoom must be between {MIN_ZOOM} and {MAX_ZOOM}"))
    }
}

pub(crate) fn check_scroll(s: Scroll) -> Result<Scroll, String> {
    if s.x.is_finite() && s.y.is_finite() && s.x >= 0.0 && s.y >= 0.0 {
        Ok(s)
    } else {
        Err("scroll offsets must be finite and >= 0".into())
    }
}

pub(crate) fn check_gain(g: f64) -> Result<f64, String> {
    if g.is_finite() && (0.0..=MAX_MASTER_GAIN).contains(&g) {
        Ok(g)
    } else {
        Err(format!(
            "master_gain must be between 0 and {MAX_MASTER_GAIN}"
        ))
    }
}

pub(crate) fn check_layout(v: &Value) -> Result<(), String> {
    // Serialization of a `Value` cannot fail.
    let len = serde_json::to_string(v).map_or(0, |s| s.len());
    if len > MAX_PANEL_LAYOUT_BYTES {
        Err(format!(
            "panel_layout is {len} bytes; the limit is {MAX_PANEL_LAYOUT_BYTES}"
        ))
    } else {
        Ok(())
    }
}

fn invalid(path: &str, msg: String) -> CmdError {
    CmdError::invalid(path, msg)
}

/// Removes duplicates, keeping first occurrences.
fn dedup(refs: Vec<SelectionRef>) -> Vec<SelectionRef> {
    let mut seen = std::collections::HashSet::new();
    refs.into_iter().filter(|r| seen.insert(*r)).collect()
}

impl ViewCtx {
    /// Default view state over `doc`.
    #[must_use]
    pub fn new(doc: Project) -> ViewCtx {
        ViewCtx {
            state: UiState::initial(),
            extra: Map::new(),
            doc,
            rev: 0,
            event_pending: false,
        }
    }

    /// Builds the state from the parsed contents of `ui-state.json`, tolerating anything:
    /// a non-object, a field of the wrong type or out of range falls back to its default, and
    /// selected IDs that no longer exist in `doc` are dropped. Unknown keys are kept for the
    /// next write. Returns the context and a warning per problem found.
    #[must_use]
    pub fn from_json(value: &Value, doc: Project) -> (ViewCtx, Vec<String>) {
        let mut ctx = ViewCtx::new(doc);
        let mut warnings = Vec::new();
        let Some(map) = value.as_object() else {
            warnings.push("ui-state.json is not a JSON object; using defaults".into());
            return (ctx, warnings);
        };
        match map.get("version").map(Value::as_u64) {
            None => {}
            Some(Some(v)) if v <= u64::from(UI_STATE_VERSION) => {}
            Some(Some(v)) => warnings.push(format!(
                "ui-state.json was written by a newer Cutweave (version {v}); unknown fields are kept"
            )),
            Some(None) => warnings.push("ui-state.json has an invalid `version`".into()),
        }
        let mut field = |key: &str, apply: &mut dyn FnMut(&Value) -> Result<(), String>| {
            if let Some(v) = map.get(key)
                && let Err(e) = apply(v)
            {
                warnings.push(format!("ui-state.json: ignoring `{key}`: {e}"));
            }
        };
        let s = &mut ctx.state;
        field("playhead", &mut |v| {
            let t: Time = from_value(v)?;
            s.playhead = check_time(t)?;
            Ok(())
        });
        field("zoom", &mut |v| {
            s.zoom = check_zoom(from_value(v)?)?;
            Ok(())
        });
        field("scroll", &mut |v| {
            s.scroll = check_scroll(from_value(v)?)?;
            Ok(())
        });
        field("preview_quality", &mut |v| {
            s.preview_quality = from_value(v)?;
            Ok(())
        });
        field("master_gain", &mut |v| {
            s.master_gain = check_gain(from_value(v)?)?;
            Ok(())
        });
        field("panel_layout", &mut |v| {
            check_layout(v)?;
            s.panel_layout = v.clone();
            Ok(())
        });
        let mut selection = Vec::new();
        field("selection", &mut |v| {
            let items = v.as_array().ok_or("expected an array")?;
            for item in items {
                // One bad entry must not lose the rest.
                if let Ok(r) = from_value::<SelectionRef>(item) {
                    selection.push(r);
                }
            }
            Ok(())
        });
        selection.retain(|r| r.check_exists(&ctx.doc).is_ok());
        selection.truncate(MAX_SELECTION);
        ctx.state.selection = dedup(selection);
        for (k, v) in map {
            if !KNOWN_KEYS.contains(&k.as_str()) {
                ctx.extra.insert(k.clone(), v.clone());
            }
        }
        (ctx, warnings)
    }

    /// The `ui-state.json` contents: unknown keys, `version` and every field.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = self.extra.clone();
        map.insert("version".into(), UI_STATE_VERSION.into());
        // Serialization of `UiState` cannot fail (finite numbers are validated on entry).
        if let Ok(Value::Object(fields)) = serde_json::to_value(&self.state) {
            map.extend(fields);
        }
        Value::Object(map)
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> &UiState {
        &self.state
    }

    /// Unknown keys preserved from the file.
    #[must_use]
    pub fn extra(&self) -> &Map<String, Value> {
        &self.extra
    }

    /// How many times the state changed since this context was created. Persistence compares
    /// it with the last written value.
    #[must_use]
    pub fn rev(&self) -> u32 {
        self.rev
    }

    /// The `view.changed` event for changes since the last call, if any.
    pub fn take_event(&mut self) -> Option<ViewChanged> {
        std::mem::take(&mut self.event_pending).then(|| ViewChanged {
            view_rev: self.rev,
            state: self.state.clone(),
        })
    }

    /// Switches to a newer document snapshot (cheap: the document shares structure) and drops
    /// selected entities that no longer exist. Returns whether the selection changed.
    pub fn set_doc(&mut self, doc: &Project) -> bool {
        self.doc = doc.clone();
        let before = self.state.selection.len();
        let doc = &self.doc;
        self.state.selection.retain(|r| r.check_exists(doc).is_ok());
        let changed = self.state.selection.len() != before;
        if changed {
            self.bump();
        }
        changed
    }

    fn bump(&mut self) {
        self.rev = self.rev.saturating_add(1);
        self.event_pending = true;
    }

    /// Applies a partial update atomically: either every field is valid and applied, or the
    /// state is untouched and the first problem is returned. Returns whether anything changed.
    pub fn apply_patch(&mut self, patch: &ViewPatch) -> Result<bool, CmdError> {
        let mut next = self.state.clone();
        if let Some(at) = patch.playhead {
            let t = at
                .resolve(None)
                .map_err(|e| invalid("/patch/playhead", e.to_string()))?;
            next.playhead = check_time(t).map_err(|m| invalid("/patch/playhead", m))?;
        }
        if let Some(z) = patch.zoom {
            next.zoom = check_zoom(z).map_err(|m| invalid("/patch/zoom", m))?;
        }
        if let Some(s) = patch.scroll {
            next.scroll = check_scroll(s).map_err(|m| invalid("/patch/scroll", m))?;
        }
        if let Some(sel) = &patch.selection {
            next.selection = self.checked_selection(sel, "/patch/selection")?;
        }
        if let Some(layout) = &patch.panel_layout {
            check_layout(layout).map_err(|m| invalid("/patch/panel_layout", m))?;
            next.panel_layout = layout.clone();
        }
        if let Some(q) = patch.preview_quality {
            next.preview_quality = q;
        }
        if let Some(g) = patch.master_gain {
            next.master_gain = check_gain(g).map_err(|m| invalid("/patch/master_gain", m))?;
        }
        Ok(self.commit(next))
    }

    /// Combines `ids` with the selection per `mode`. Added IDs must exist; removing unknown
    /// ones is harmless.
    pub fn select(&mut self, ids: &[SelectionRef], mode: SelectMode) -> Result<bool, CmdError> {
        let mut next = self.state.clone();
        let current = &self.state.selection;
        next.selection = match mode {
            SelectMode::Replace => self.checked_selection(ids, "/ids")?,
            SelectMode::Add => {
                let mut all = current.clone();
                all.extend(self.checked_selection(ids, "/ids")?);
                self.checked_len(dedup(all), "/ids")?
            }
            SelectMode::Remove => current
                .iter()
                .copied()
                .filter(|r| !ids.contains(r))
                .collect(),
            SelectMode::Toggle => {
                let ids = dedup(ids.to_vec());
                let added: Vec<_> = ids
                    .iter()
                    .copied()
                    .filter(|r| !current.contains(r))
                    .collect();
                self.checked_selection(&added, "/ids")?;
                let mut all: Vec<_> = current
                    .iter()
                    .copied()
                    .filter(|r| !ids.contains(r))
                    .collect();
                all.extend(added);
                self.checked_len(all, "/ids")?
            }
        };
        Ok(self.commit(next))
    }

    /// Sets the playhead (a `view.set_playhead`).
    pub fn set_playhead(&mut self, at: TimeInput) -> Result<bool, CmdError> {
        let t = at
            .resolve(None)
            .map_err(|e| invalid("/at", e.to_string()))?;
        let mut next = self.state.clone();
        next.playhead = check_time(t).map_err(|m| invalid("/at", m))?;
        Ok(self.commit(next))
    }

    /// Sets the zoom (a `view.set_zoom`).
    pub fn set_zoom(&mut self, zoom: f64) -> Result<bool, CmdError> {
        let mut next = self.state.clone();
        next.zoom = check_zoom(zoom).map_err(|m| invalid("/zoom", m))?;
        Ok(self.commit(next))
    }

    fn checked_selection(
        &self,
        refs: &[SelectionRef],
        path: &str,
    ) -> Result<Vec<SelectionRef>, CmdError> {
        for r in refs {
            r.check_exists(&self.doc)?;
        }
        self.checked_len(dedup(refs.to_vec()), path)
    }

    #[allow(clippy::unused_self)] // keeps the call sites uniform
    fn checked_len(
        &self,
        refs: Vec<SelectionRef>,
        path: &str,
    ) -> Result<Vec<SelectionRef>, CmdError> {
        if refs.len() > MAX_SELECTION {
            Err(invalid(
                path,
                format!("at most {MAX_SELECTION} entities can be selected"),
            ))
        } else {
            Ok(refs)
        }
    }

    fn commit(&mut self, next: UiState) -> bool {
        if next == self.state {
            return false;
        }
        self.state = next;
        self.bump();
        true
    }
}

fn from_value<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}
