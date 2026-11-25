// SPDX-License-Identifier: GPL-3.0-or-later
//! Autosave, rotating backups and the project file events (WP-1C.03; arch §5.2, §17).
//!
//! [`AutosaveService`] saves the **committed** document through the store's atomic path when it
//! is dirty. *Dirty* means `last_saved_rev != committed_rev` ([`AutosaveService::is_dirty`]);
//! an open gesture or proposal never makes it dirty and is never written (the service reads
//! [`DocHandle::committed_snapshot`]).
//!
//! # When it fires
//!
//! - **Idle:** no commit for [`AutosaveConfig::idle`] (2 s).
//! - **Continuous edits:** at least every [`AutosaveConfig::interval`] (30 s,
//!   `settings.autosave.interval_s`) since the document became dirty.
//! - A failed attempt is retried after [`AutosaveConfig::retry`], with a
//!   `project.autosave_failed` event.
//! - Nothing happens while disabled, read-only or while a crash recovery awaits a decision.
//!
//! # Threading and time
//!
//! The decision logic is [`AutosaveService::tick`], a plain function of the injected
//! [`Clock`]: tests drive it with a `ManualClock` and never sleep. [`AutosaveService::spawn`]
//! runs it on a dedicated thread every [`AutosaveConfig::poll`] (a condvar wait, so shutdown is
//! immediate). The thread reads the lock-free committed snapshot and then serializes and
//! `fsync`s on its own; it shares only the journal's short-held lock with the doc actor (a
//! segment rotation), so command acks are not delayed (bench `autosave`).
//!
//! # Backups
//!
//! After an autosave, if the saved document's SHA-256 differs from the newest backup, it is
//! copied to `.cutweave/backups/autosave-<UTC stamp>.cutweave` and all but the newest
//! [`AutosaveConfig::keep_backups`] (10) are deleted. Explicit saves ([`AutosaveService::save_now`])
//! do not rotate backups.

mod backups;
mod events;

pub(crate) use backups::civil_from_days;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use cutweave_commands::{Clock, DocHandle, Rev, WallTime};
use serde::{Deserialize, Serialize};

pub use events::{
    AutosaveFailed, PROJECT_EVENTS, ProjectDirty, ProjectEvent, ProjectSaved, RecoveryPayload,
};

use crate::project_io::{ProjectIoError, ProjectStore, document_sha256};

/// The project store shared between the autosave thread and the engine root (explicit save,
/// save-as, close).
pub type SharedStore = Arc<Mutex<ProjectStore>>;

/// `settings.autosave` as persisted in the project document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutosaveSettings {
    /// Whether autosave runs.
    pub enabled: bool,
    /// The longest time, in seconds, a dirty document goes unsaved during continuous edits.
    pub interval_s: u32,
}

impl Default for AutosaveSettings {
    fn default() -> Self {
        AutosaveSettings {
            enabled: true,
            interval_s: 30,
        }
    }
}

/// The app's own autosave preference (`project.set_autosave`, arch §17): when an editor has
/// set one, it wins over the project's `settings.autosave` for every project that engine opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutosavePreference {
    /// Whether autosave runs.
    pub enabled: bool,
    /// The longest time, in seconds, a dirty document goes unsaved during continuous edits.
    pub interval_s: u32,
    /// Backups kept in `.cutweave/backups/`.
    pub keep_backups: u32,
}

impl AutosaveConfig {
    /// This configuration with the app's preference applied (idle, poll and retry stay).
    #[must_use]
    pub fn with_preference(self, pref: AutosavePreference) -> AutosaveConfig {
        AutosaveConfig {
            enabled: pref.enabled,
            interval: Duration::from_secs(u64::from(pref.interval_s.max(1))),
            keep_backups: usize::try_from(pref.keep_backups.max(1)).unwrap_or(usize::MAX),
            ..self
        }
    }
}

/// Autosave timing and retention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutosaveConfig {
    /// Whether autosave runs.
    pub enabled: bool,
    /// Save at least this often while edits continue.
    pub interval: Duration,
    /// Save once there were no commits for this long.
    pub idle: Duration,
    /// Backups kept in `.cutweave/backups/`.
    pub keep_backups: usize,
    /// How often the background thread evaluates [`AutosaveService::tick`].
    pub poll: Duration,
    /// Wait before retrying a failed save.
    pub retry: Duration,
}

impl Default for AutosaveConfig {
    fn default() -> Self {
        AutosaveConfig::from(AutosaveSettings::default())
    }
}

impl From<AutosaveSettings> for AutosaveConfig {
    fn from(s: AutosaveSettings) -> Self {
        AutosaveConfig {
            enabled: s.enabled,
            interval: Duration::from_secs(u64::from(s.interval_s.max(1))),
            idle: Duration::from_secs(2),
            keep_backups: 10,
            poll: Duration::from_millis(250),
            retry: Duration::from_secs(5),
        }
    }
}

/// What one [`AutosaveService::tick`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// Nothing to save.
    Clean,
    /// Dirty, but neither the idle nor the interval deadline has passed (or a retry is pending).
    Waiting,
    /// Autosave is off.
    Disabled,
    /// The store is read-only or a crash recovery awaits a decision: nothing may be written.
    Unavailable,
    /// Saved; `backed_up` says whether a new backup was written (the content changed).
    Saved {
        /// The committed revision written.
        rev: Rev,
        /// Whether a backup copy was made.
        backed_up: bool,
    },
    /// The save failed (a `project.autosave_failed` event was sent).
    Failed,
}

#[derive(Debug)]
struct State {
    observed_rev: Rev,
    last_change: WallTime,
    dirty_since: Option<WallTime>,
    published_dirty: bool,
    failed_at: Option<WallTime>,
    backup_dir: Option<PathBuf>,
    last_backup_sha: Option<String>,
}

struct Inner {
    doc: DocHandle,
    store: SharedStore,
    clock: Arc<dyn Clock>,
    config: Mutex<AutosaveConfig>,
    saved_rev: AtomicU64,
    state: Mutex<State>,
    subscribers: Mutex<Vec<Sender<ProjectEvent>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Autosaves the committed document of a doc actor into its project store. Cheap to clone.
#[derive(Clone)]
pub struct AutosaveService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AutosaveService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutosaveService")
            .field("saved_rev", &self.saved_rev())
            .finish_non_exhaustive()
    }
}

impl AutosaveService {
    /// A service over `doc` and `store`. `saved_rev` is the committed revision the document on
    /// disk corresponds to: the doc's initial revision after a plain open, or the journal's
    /// base revision ([`RecoveryPayload::base_rev`]) after a restore. No thread runs until
    /// [`spawn`](Self::spawn); tests call [`tick`](Self::tick) themselves.
    #[must_use]
    pub fn new(
        doc: DocHandle,
        store: SharedStore,
        clock: Arc<dyn Clock>,
        config: AutosaveConfig,
        saved_rev: Rev,
    ) -> AutosaveService {
        let now = clock.now();
        let observed_rev = doc.committed_snapshot().committed_rev;
        // A restored document starts dirty (the journal's edits are not on disk). Clients learn
        // that from the status the engine sends with `project.opened`, not from a `project.dirty`
        // event, so what was published is that state: the save that clears it must then publish
        // `dirty: false`, even when it comes before the first tick (issue #145).
        let published_dirty = observed_rev != saved_rev;
        AutosaveService {
            inner: Arc::new(Inner {
                doc,
                store,
                clock,
                config: Mutex::new(config),
                saved_rev: AtomicU64::new(saved_rev.0),
                state: Mutex::new(State {
                    observed_rev,
                    last_change: now,
                    dirty_since: None,
                    published_dirty,
                    failed_at: None,
                    backup_dir: None,
                    last_backup_sha: None,
                }),
                subscribers: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The revision last written to the project document.
    #[must_use]
    pub fn saved_rev(&self) -> Rev {
        Rev(self.inner.saved_rev.load(Ordering::Acquire))
    }

    /// Whether the committed revision differs from the last saved one (live, not as of the
    /// last tick).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.inner.doc.committed_snapshot().committed_rev != self.saved_rev()
    }

    /// Replaces the timing and retention (`settings.autosave` changed).
    pub fn set_config(&self, config: AutosaveConfig) {
        *lock(&self.inner.config) = config;
    }

    /// Applies the app's preference to the running configuration at once: an interval change
    /// counts for the current dirty period, disabling stops further autosaves.
    pub fn apply_preference(&self, pref: AutosavePreference) {
        let mut cfg = lock(&self.inner.config);
        *cfg = cfg.with_preference(pref);
    }

    /// The current configuration.
    #[must_use]
    pub fn config(&self) -> AutosaveConfig {
        *lock(&self.inner.config)
    }

    /// Subscribes to the project file events. The receiver sees events sent after this call.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<ProjectEvent> {
        let (tx, rx) = mpsc::channel();
        lock(&self.inner.subscribers).push(tx);
        rx
    }

    /// Records that the engine saved the document itself (save-as, or a save outside this
    /// service) at committed revision `rev`.
    pub fn note_saved(&self, rev: Rev) {
        self.inner.saved_rev.store(rev.0, Ordering::Release);
        let mut st = lock(&self.inner.state);
        // The backups folder may have changed (save-as).
        st.backup_dir = None;
        st.last_backup_sha = None;
        st.failed_at = None;
        self.refresh_dirty(&mut st, self.inner.clock.now());
    }

    // ----- recovery (WP-1M.07 reads these; restore/discard stay ProjectStore methods) --------

    /// What a restore would bring back, if unsaved work from a previous session awaits a
    /// decision ([`ProjectStore::recover`] / [`ProjectStore::discard_recovery`]).
    #[must_use]
    pub fn recovery_available(&self) -> Option<RecoveryPayload> {
        lock(&self.inner.store)
            .recovery()
            .map(|r| RecoveryPayload::from(&r))
    }

    /// Sends `project.recovery_available` if a recovery awaits a decision. Returns whether it
    /// did.
    #[allow(clippy::must_use_candidate)]
    pub fn announce_recovery(&self) -> bool {
        match self.recovery_available() {
            Some(payload) => {
                self.emit(&ProjectEvent::RecoveryAvailable(payload));
                true
            }
            None => false,
        }
    }

    // ----- saving ------------------------------------------------------------------------------

    /// Saves the committed document now (an explicit save): no backup rotation. Emits
    /// `project.saved` and `project.dirty`.
    ///
    /// # Errors
    ///
    /// The store's error (read-only, recovery pending, I/O).
    pub fn save_now(&self) -> Result<Rev, ProjectIoError> {
        let mut st = lock(&self.inner.state);
        let snap = self.inner.doc.committed_snapshot();
        let now = self.inner.clock.now();
        self.write(&mut st, &snap, now, false).map(|(rev, _)| rev)
    }

    /// Evaluates the autosave rules at `now` and saves if due. The background thread calls this
    /// every [`AutosaveConfig::poll`]; tests call it with a manual clock.
    pub fn tick(&self, now: WallTime) -> TickOutcome {
        let cfg = self.config();
        let mut st = lock(&self.inner.state);
        let snap = self.inner.doc.committed_snapshot();
        if snap.committed_rev != st.observed_rev {
            st.observed_rev = snap.committed_rev;
            st.last_change = now;
        }
        let dirty = self.refresh_dirty(&mut st, now);
        if !dirty {
            return TickOutcome::Clean;
        }
        if !cfg.enabled {
            return TickOutcome::Disabled;
        }
        let idle_for = now.saturating_since(st.last_change);
        let dirty_for = st
            .dirty_since
            .map_or(Duration::ZERO, |t| now.saturating_since(t));
        if idle_for < cfg.idle && dirty_for < cfg.interval {
            return TickOutcome::Waiting;
        }
        if st
            .failed_at
            .is_some_and(|t| now.saturating_since(t) < cfg.retry)
        {
            return TickOutcome::Waiting;
        }
        {
            let store = lock(&self.inner.store);
            if store.read_only().is_some() || store.recovery().is_some() {
                return TickOutcome::Unavailable;
            }
        }
        match self.write(&mut st, &snap, now, true) {
            Ok((rev, backed_up)) => TickOutcome::Saved { rev, backed_up },
            Err(e) => {
                tracing::warn!(error = %e, "autosave failed");
                st.failed_at = Some(now);
                drop(st);
                self.emit(&ProjectEvent::AutosaveFailed(AutosaveFailed {
                    message: e.to_string(),
                }));
                TickOutcome::Failed
            }
        }
    }

    /// Runs [`tick`](Self::tick) on a dedicated thread until the returned task is dropped.
    ///
    /// # Errors
    ///
    /// The thread could not be started.
    pub fn spawn(&self) -> io::Result<AutosaveTask> {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let service = self.clone();
        let flag = Arc::clone(&stop);
        let join = std::thread::Builder::new()
            .name("cutweave-autosave".into())
            .spawn(move || {
                loop {
                    let poll = service.config().poll;
                    {
                        let (stopped, cv) = &*flag;
                        let guard = lock(stopped);
                        let (guard, _) = cv
                            .wait_timeout_while(guard, poll, |s| !*s)
                            .unwrap_or_else(PoisonError::into_inner);
                        if *guard {
                            return;
                        }
                    }
                    service.tick(service.inner.clock.now());
                }
            })?;
        Ok(AutosaveTask {
            stop,
            join: Some(join),
        })
    }

    // ----- internals ---------------------------------------------------------------------------

    /// Recomputes dirtiness from the committed revision, tracks when it began and publishes
    /// `project.dirty` on a change.
    fn refresh_dirty(&self, st: &mut State, now: WallTime) -> bool {
        let committed = self.inner.doc.committed_snapshot().committed_rev;
        let dirty = committed != self.saved_rev();
        if dirty {
            st.dirty_since.get_or_insert(now);
        } else {
            st.dirty_since = None;
        }
        if dirty != st.published_dirty {
            st.published_dirty = dirty;
            self.emit(&ProjectEvent::Dirty(ProjectDirty { dirty }));
        }
        dirty
    }

    fn write(
        &self,
        st: &mut State,
        snap: &cutweave_commands::DocSnapshot,
        now: WallTime,
        backup: bool,
    ) -> Result<(Rev, bool), ProjectIoError> {
        let mut store = lock(&self.inner.store);
        store.save_snapshot(snap)?;
        let path = store.document_path().to_string_lossy().into_owned();
        let mut backed_up = false;
        if backup {
            backed_up = self.rotate_backup(st, &store, now);
        }
        drop(store);
        self.inner
            .saved_rev
            .store(snap.committed_rev.0, Ordering::Release);
        st.failed_at = None;
        st.dirty_since = None;
        self.emit(&ProjectEvent::Saved(ProjectSaved {
            rev: snap.committed_rev,
            path,
        }));
        self.refresh_dirty(st, now);
        Ok((snap.committed_rev, backed_up))
    }

    /// Copies the document just saved into the backups folder if its content differs from the
    /// newest backup. A backup failure is logged, never fatal: the save itself succeeded.
    fn rotate_backup(&self, st: &mut State, store: &ProjectStore, now: WallTime) -> bool {
        let dir = store.layout().backups_dir();
        if st.backup_dir.as_ref() != Some(&dir) {
            st.last_backup_sha = backups::list(&dir)
                .last()
                .and_then(|newest| std::fs::read(newest).ok())
                .map(|bytes| document_sha256(&bytes));
            st.backup_dir = Some(dir.clone());
        }
        let Some(sha) = store.saved_sha256() else {
            return false;
        };
        if st.last_backup_sha.as_deref() == Some(sha) {
            return false;
        }
        let keep = self.config().keep_backups;
        let result = std::fs::read(store.document_path())
            .and_then(|bytes| backups::add(&dir, now, &bytes, keep));
        match result {
            Ok(_) => {
                st.last_backup_sha = Some(sha.to_owned());
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not write an autosave backup");
                false
            }
        }
    }

    fn emit(&self, event: &ProjectEvent) {
        lock(&self.inner.subscribers).retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// The background autosave thread; stops and joins when dropped.
#[derive(Debug)]
pub struct AutosaveTask {
    stop: Arc<(Mutex<bool>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl AutosaveTask {
    /// Stops the thread and waits for it (also done on drop).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let (stopped, cv) = &*self.stop;
        *lock(stopped) = true;
        cv.notify_all();
        if let Some(join) = self.join.take()
            && join.join().is_err()
        {
            tracing::error!("the autosave thread panicked");
        }
    }
}

impl Drop for AutosaveTask {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The autosave backups in a project's `.cutweave/backups/` folder, oldest first (other files
/// there, such as pre-migration originals, are not listed).
#[must_use]
pub fn list_backups(dir: &std::path::Path) -> Vec<PathBuf> {
    backups::list(dir)
}
