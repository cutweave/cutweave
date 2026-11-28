// SPDX-License-Identifier: GPL-3.0-or-later
//! [`Journal`]: the append-only writer behind the doc actor's [`CommitSink`], its durability
//! policy ([`SyncPolicy`], group commit on a flusher thread) and segment rotation on save.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cutweave_commands::{
    CheckpointId, CheckpointLedger, CheckpointRecord, Clock, CmdError, CommitRecord, CommitSink,
    DEDUP_CAPACITY, DEDUP_WINDOW, DedupRecord, JournaledCommit, Patch, Registry, Rev, SystemClock,
    TxMarker, WallTime, diff,
};
use cutweave_model::Project;

use super::{
    JournalError,
    format::{
        CarriedCheckpoint, Carry, CommitEntry, Entry, FORMAT_VERSION, MAGIC, SegmentHeader,
        checkpoint_frame, commit_frame, entry_frame, tx_frame,
    },
    ids::IdTracker,
};

/// The default group-commit delay: a committed transaction reaches stable storage at most this
/// long (plus one `fdatasync`) after it is acknowledged.
pub const DEFAULT_GROUP_COMMIT: Duration = Duration::from_millis(50);

/// When journal writes are made durable (`fdatasync`).
///
/// Every policy writes each transaction to the OS (`write()`) **before** the doc actor
/// acknowledges it, so an acknowledged transaction survives the engine process being killed
/// at any point. The policy only decides how long it may sit in the OS page cache, which
/// matters for power loss and OS crashes. Rotation (every save), [`Journal::sync`] and
/// [`Journal::close`] always `fdatasync`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Group commit (the default, [`DEFAULT_GROUP_COMMIT`]): a flusher thread `fdatasync`s
    /// within `max_delay` of the first unsynced write, off the actor thread, so acks never
    /// wait for the disk. Power-loss window: `max_delay` plus one `fdatasync`.
    Group {
        /// Longest time a written transaction stays unsynced.
        max_delay: Duration,
    },
    /// `fdatasync` before every acknowledgement: no power-loss window, at the cost of one
    /// device flush per committed transaction on the actor thread.
    EveryCommit,
    /// Only on rotation, [`Journal::sync`] and [`Journal::close`] (tests and benchmarks).
    Manual,
}

impl Default for SyncPolicy {
    fn default() -> SyncPolicy {
        SyncPolicy::Group {
            max_delay: DEFAULT_GROUP_COMMIT,
        }
    }
}

/// How to run a [`Journal`].
#[derive(Clone)]
pub struct JournalConfig {
    sync: SyncPolicy,
    clock: Arc<dyn Clock>,
    dedup_window: Duration,
    registry: Option<Arc<Registry>>,
    ids: Option<IdTracker>,
}

impl fmt::Debug for JournalConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalConfig")
            .field("sync", &self.sync)
            .field("clock", &self.clock)
            .field("dedup_window", &self.dedup_window)
            .field("ids", &self.ids)
            .finish_non_exhaustive()
    }
}

impl Default for JournalConfig {
    fn default() -> JournalConfig {
        JournalConfig::new(Arc::new(SystemClock))
    }
}

impl JournalConfig {
    /// Group commit ([`SyncPolicy::default`]), the 10-minute dedup window, wall time from
    /// `clock` (pass the doc actor's clock).
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> JournalConfig {
        JournalConfig {
            sync: SyncPolicy::default(),
            clock,
            dedup_window: DEDUP_WINDOW,
            registry: None,
            ids: None,
        }
    }

    /// The durability policy.
    #[must_use]
    pub fn with_sync(mut self, sync: SyncPolicy) -> JournalConfig {
        self.sync = sync;
        self
    }

    /// How long requests are remembered across a save (the doc actor's dedup window).
    #[must_use]
    pub fn with_dedup_window(mut self, window: Duration) -> JournalConfig {
        self.dedup_window = window;
        self
    }

    /// The registry, so the journal can enforce `#[secret]` redaction itself (arch §14): a
    /// step whose secret params are not redacted is redacted before it is written.
    #[must_use]
    pub fn with_registry(mut self, registry: Arc<Registry>) -> JournalConfig {
        self.registry = Some(registry);
        self
    }

    /// The deterministic id streams (`--deterministic-ids`): their positions are journaled
    /// with every transaction and the seed in every segment header.
    #[must_use]
    pub fn with_ids(mut self, ids: IdTracker) -> JournalConfig {
        self.ids = Some(ids);
        self
    }

    /// The durability policy.
    #[must_use]
    pub fn sync_policy(&self) -> SyncPolicy {
        self.sync
    }

    pub(crate) fn now(&self) -> WallTime {
        self.clock.now()
    }
}

/// Counters, for diagnostics and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStats {
    /// Transactions appended.
    pub appends: u64,
    /// Bytes appended (frames).
    pub bytes: u64,
    /// `fdatasync` calls that succeeded.
    pub syncs: u64,
    /// Rotations (saves).
    pub rotations: u64,
}

/// What a rotation superseded; hand it to [`Journal::retire`] once the document is saved.
#[derive(Debug)]
#[must_use = "retire the superseded segments after the document is saved"]
pub struct Rotation {
    superseded: Vec<PathBuf>,
}

/// Where a new journal starts.
#[derive(Debug)]
pub(crate) enum Start {
    /// A new segment over the saved document.
    Fresh {
        base_rev: Rev,
        doc_sha256: String,
        generation: u64,
        requests: Vec<DedupRecord>,
        retained: Vec<PathBuf>,
    },
    /// Continue appending to an existing segment (after recovery).
    Resume {
        path: PathBuf,
        header: SegmentHeader,
        valid_len: u64,
        revs: Vec<(Rev, u64)>,
        last_rev: Rev,
        requests: Vec<DedupRecord>,
        chain: Vec<PathBuf>,
        retained: Vec<PathBuf>,
        next_generation: u64,
        /// The checkpoints the recovery rebuilt (to carry over the next save).
        checkpoints: Box<CheckpointLedger>,
        /// The highest transaction id the journal said may be handed out (ADR-0043).
        tx_issued: u64,
    },
}

impl Start {
    /// The checkpoints to start with (none for a new segment).
    fn take_checkpoints(&mut self) -> CheckpointLedger {
        match self {
            Start::Fresh { .. } => CheckpointLedger::new(),
            Start::Resume { checkpoints, .. } => std::mem::take(&mut **checkpoints),
        }
    }
}

/// The append-only command journal of one open project (WP-1C.02; arch §5.1, §17).
///
/// Cheap to clone (a handle). [`sink`](Self::sink) is what the doc actor writes through
/// ([`DocConfig::with_journal`](cutweave_commands::DocConfig::with_journal)); the project store
/// keeps a handle to [`rotate`](Self::rotate) it on save. When the last handle is dropped the
/// journal is synced and its flusher thread stops (as by [`close`](Self::close)).
#[derive(Clone)]
pub struct Journal {
    handle: Arc<Handle>,
}

impl fmt::Debug for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let st = self.handle.shared.lock();
        f.debug_struct("Journal")
            .field("segment", &st.seg.path)
            .field("base_rev", &st.seg.header.base_rev)
            .field("last_rev", &st.last_rev)
            .field("sync", &self.handle.shared.sync)
            .finish_non_exhaustive()
    }
}

struct Handle {
    shared: Arc<Shared>,
    flusher: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Err(e) = self.shutdown() {
            tracing::error!(error = %e, "journal did not close cleanly");
        }
    }
}

impl Handle {
    fn shutdown(&self) -> Result<(), JournalError> {
        {
            let mut st = self.shared.lock();
            if st.closed {
                return Ok(());
            }
            st.closed = true;
        }
        self.shared.wake.notify_all();
        let flusher = self
            .flusher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = flusher
            && thread.join().is_err()
        {
            tracing::error!("journal flusher thread panicked");
        }
        let (file, path, upto) = {
            let mut st = self.shared.lock();
            st.dirty_since = None;
            if st.synced == st.written {
                return Ok(());
            }
            (
                Arc::clone(&st.seg.sync_file),
                st.seg.path.clone(),
                st.written,
            )
        };
        file.sync_data()
            .map_err(|e| JournalError::io("sync journal", &path, e))?;
        let mut st = self.shared.lock();
        st.synced = st.synced.max(upto);
        st.stats.syncs += 1;
        Ok(())
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    sync: SyncPolicy,
    clock: Arc<dyn Clock>,
    window: Duration,
    registry: Option<Arc<Registry>>,
    ids: Option<IdTracker>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct Segment {
    path: PathBuf,
    /// The handle appends go through.
    file: File,
    /// A second handle (its own file object) the flusher syncs through, so an `fdatasync`
    /// never serializes with an append on the same handle (Windows synchronous I/O).
    sync_file: Arc<File>,
    header: SegmentHeader,
    len: u64,
    /// Each transaction's committed revision and frame offset, for carrying the newest ones
    /// over to the next segment on rotation.
    revs: Vec<(Rev, u64)>,
}

struct State {
    dir: PathBuf,
    seg: Segment,
    /// Older segments of the current chain (still needed until the next successful save).
    chain: Vec<PathBuf>,
    /// The journal of the last save, kept until the next successful save.
    retained: Vec<PathBuf>,
    next_generation: u64,
    last_rev: Rev,
    /// Requests of recent transactions, carried into the next segment on rotation so the
    /// dedup window survives saves and restarts.
    requests: VecDeque<DedupRecord>,
    /// The checkpoints (WP-2I.03), mirrored from their markers with their documents (shared
    /// with the doc actor's): the ones older than a save are carried into the next segment.
    checkpoints: CheckpointLedger,
    /// The highest transaction id a marker said may be handed out (ADR-0043), carried into
    /// the next segment on rotation: ids are never reused.
    tx_issued: u64,
    dirty_since: Option<Instant>,
    last_sync: Instant,
    /// Appends so far, and how many of them are known durable.
    written: u64,
    synced: u64,
    /// A write or sync failed and the segment may not hold every acknowledged transaction:
    /// further commits are refused until a rotation (save) starts a clean segment.
    error: Option<String>,
    closed: bool,
    stats: JournalStats,
}

impl State {
    fn writable(&self) -> Result<(), JournalError> {
        if self.closed {
            return Err(JournalError::Closed);
        }
        if let Some(e) = &self.error {
            return Err(JournalError::Failed(e.clone()));
        }
        Ok(())
    }

    fn remember(
        &mut self,
        records: impl Iterator<Item = DedupRecord>,
        now: WallTime,
        window: Duration,
    ) {
        self.requests.extend(records);
        while self
            .requests
            .front()
            .is_some_and(|r| !r.is_live(now, window))
            || self.requests.len() > DEDUP_CAPACITY
        {
            self.requests.pop_front();
        }
    }
}

fn open_rw(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open(path)
}

fn open_sync(path: &Path) -> io::Result<File> {
    // `FlushFileBuffers` needs write access; `fsync` works on any descriptor of the file.
    OpenOptions::new().write(true).open(path)
}

/// `<dir>/<base-rev>.jnl`, or `<base-rev>-<k>.jnl` when taken.
fn segment_path(dir: &Path, base_rev: Rev) -> PathBuf {
    let first = dir.join(format!("{base_rev}.jnl"));
    if !first.exists() {
        return first;
    }
    (1..u32::MAX)
        .map(|k| dir.join(format!("{base_rev}-{k}.jnl")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Makes a directory entry durable (best effort; see `project_io::atomic`).
pub(crate) fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        if let Ok(d) = OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
        {
            let _ = d.sync_all();
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = dir;
}

fn read_range(path: &Path, from: u64, to: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let len = usize::try_from(to.saturating_sub(from)).map_err(io::Error::other)?;
    let mut out = vec![0; len];
    file.read_exact(&mut out)?;
    Ok(out)
}

/// The first frames of a new segment: magic, header, what it carries over.
fn segment_prefix(header: &SegmentHeader, carry: Carry) -> Result<Vec<u8>, JournalError> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(entry_frame(&Entry::Header(header.clone()))?);
    if !carry.is_empty() {
        bytes.extend(entry_frame(&Entry::Carry(carry))?);
    }
    Ok(bytes)
}

/// The patches that turn `base` (the document a new segment starts from, at `base_rev`) into
/// each checkpoint of `ledger` made at or before `base_rev`; a later one is in the transactions
/// the new segment repeats, so replay rebuilds it. Already computed ones in `known` are reused.
fn carried_patches(
    ledger: &CheckpointLedger,
    base_rev: Rev,
    base: &Project,
    known: &mut BTreeMap<CheckpointId, Option<Patch>>,
) {
    for entry in ledger.iter().filter(|e| e.info.rev <= base_rev) {
        known.entry(entry.info.checkpoint).or_insert_with(|| {
            diff(base, &entry.doc)
                .map_err(|e| {
                    tracing::warn!(
                        checkpoint = %entry.info.checkpoint,
                        error = %e,
                        "a checkpoint cannot be carried over the save; it is lost after a restart"
                    );
                })
                .ok()
        });
    }
}

/// What a new segment over `base_rev` (the document `base`, when given) carries over: the
/// remembered requests and the checkpoints made at or before `base_rev` (their patches in
/// `patches`, completed here for ones made since they were computed).
fn carry_over(
    st: &State,
    base_rev: Rev,
    base: Option<&Project>,
    patches: &mut BTreeMap<CheckpointId, Option<Patch>>,
) -> Carry {
    match base {
        Some(base) => carried_patches(&st.checkpoints, base_rev, base, patches),
        None if st.checkpoints.iter().any(|e| e.info.rev <= base_rev) => {
            tracing::warn!(
                %base_rev,
                "journal rotated without the saved document: older checkpoints are not carried over"
            );
        }
        None => {}
    }
    Carry {
        requests: st.requests.iter().cloned().collect(),
        checkpoints: carried(&st.checkpoints, base_rev, patches),
        checkpoints_issued: st.checkpoints.issued(),
        tx_issued: st.tx_issued,
    }
}

/// The checkpoints of `ledger` to carry into a segment over `base_rev`, from `patches`.
fn carried(
    ledger: &CheckpointLedger,
    base_rev: Rev,
    patches: &BTreeMap<CheckpointId, Option<Patch>>,
) -> Vec<CarriedCheckpoint> {
    ledger
        .iter()
        .filter(|e| e.info.rev <= base_rev)
        .filter_map(|e| {
            let patch = patches.get(&e.info.checkpoint)?.clone()?;
            Some(CarriedCheckpoint {
                info: e.info.clone(),
                patch,
            })
        })
        .collect()
}

/// Creates `path` holding `bytes` (not yet synced) and opens both handles.
fn create_segment(path: &Path, bytes: &[u8]) -> Result<(File, File), JournalError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| JournalError::io("create journal segment", path, e))?;
    if let Err(e) = file.write_all(bytes) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(JournalError::io("write journal segment", path, e));
    }
    let sync_file = open_sync(path).map_err(|e| JournalError::io("open journal", path, e))?;
    Ok((file, sync_file))
}

/// A new segment in `dir` holding `header` and `requests`, synced.
fn fresh_segment(
    dir: &Path,
    header: SegmentHeader,
    requests: Vec<DedupRecord>,
) -> Result<Segment, JournalError> {
    let bytes = segment_prefix(
        &header,
        Carry {
            requests,
            ..Carry::default()
        },
    )?;
    let path = segment_path(dir, header.base_rev);
    let (file, sync_file) = create_segment(&path, &bytes)?;
    sync_file
        .sync_data()
        .map_err(|e| JournalError::io("sync journal", &path, e))?;
    sync_dir(dir);
    Ok(Segment {
        path,
        file,
        sync_file: Arc::new(sync_file),
        header,
        len: bytes.len() as u64,
        revs: Vec::new(),
    })
}

/// The existing segment `path`, cut back to `valid_len` (its last valid entry), and whether
/// anything was cut.
fn resume_segment(
    path: PathBuf,
    header: SegmentHeader,
    valid_len: u64,
    revs: Vec<(Rev, u64)>,
) -> Result<(Segment, bool), JournalError> {
    let file = open_rw(&path).map_err(|e| JournalError::io("open journal", &path, e))?;
    let on_disk = file
        .metadata()
        .map_err(|e| JournalError::io("stat journal", &path, e))?
        .len();
    if on_disk != valid_len {
        // Drop the torn or corrupt tail (the caller preserved a copy if needed), so new
        // transactions follow the last valid one.
        file.set_len(valid_len)
            .map_err(|e| JournalError::io("truncate journal", &path, e))?;
        file.sync_data()
            .map_err(|e| JournalError::io("sync journal", &path, e))?;
    }
    let sync_file = open_sync(&path).map_err(|e| JournalError::io("open journal", &path, e))?;
    let segment = Segment {
        path,
        file,
        sync_file: Arc::new(sync_file),
        header,
        len: valid_len,
        revs,
    };
    Ok((segment, on_disk != valid_len))
}

impl Journal {
    /// Starts appending (see [`Start`]).
    pub(crate) fn start(
        dir: &Path,
        mut start: Start,
        config: JournalConfig,
    ) -> Result<Journal, JournalError> {
        let now = config.clock.now();
        let checkpoints = start.take_checkpoints();
        let tx_issued = match &start {
            Start::Fresh { .. } => 0,
            Start::Resume { tx_issued, .. } => *tx_issued,
        };
        let mut cut = false;
        let (seg, chain, retained, next_generation, last_rev, requests) = match start {
            Start::Fresh {
                base_rev,
                doc_sha256,
                generation,
                requests,
                retained,
            } => {
                let requests: Vec<DedupRecord> = requests
                    .into_iter()
                    .filter(|r| r.is_live(now, config.dedup_window))
                    .collect();
                let header = SegmentHeader {
                    format: FORMAT_VERSION,
                    generation,
                    base_rev,
                    doc_sha256,
                    created_at: now,
                    seed: config.ids.as_ref().map(IdTracker::seed),
                    ids: config.ids.as_ref().map(IdTracker::state),
                };
                let seg = fresh_segment(dir, header, requests.clone())?;
                (
                    seg,
                    Vec::new(),
                    retained,
                    generation + 1,
                    base_rev,
                    requests,
                )
            }
            Start::Resume {
                path,
                header,
                valid_len,
                revs,
                last_rev,
                requests,
                chain,
                retained,
                next_generation,
                ..
            } => {
                let (seg, truncated) = resume_segment(path, header, valid_len, revs)?;
                cut = truncated;
                (seg, chain, retained, next_generation, last_rev, requests)
            }
        };
        let mut seg = seg;
        seg.file
            .seek(SeekFrom::Start(seg.len))
            .map_err(|e| JournalError::io("seek journal", &seg.path, e))?;
        let mut state = State {
            dir: dir.to_path_buf(),
            seg,
            chain,
            retained,
            next_generation,
            last_rev,
            requests: VecDeque::new(),
            checkpoints,
            tx_issued,
            dirty_since: None,
            last_sync: Instant::now(),
            written: 0,
            synced: 0,
            error: None,
            closed: false,
            stats: JournalStats::default(),
        };
        state.remember(requests.into_iter(), now, config.dedup_window);
        if cut && tx_issued > 0 {
            // The cut tail (discarded work, or a torn record) may have held the marker that
            // said which transaction ids were handed out: say it again, so they stay taken
            // if this engine dies before it opens a transaction.
            let marker = TxMarker {
                issued: tx_issued,
                at: now,
                closed: None,
                steps: &[],
            };
            let frame = tx_frame(&marker, config.ids.as_ref().map(IdTracker::state))?;
            write_frame(&mut state, &frame)?;
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            wake: Condvar::new(),
            sync: config.sync,
            clock: config.clock,
            window: config.dedup_window,
            registry: config.registry,
            ids: config.ids,
        });
        let flusher = match config.sync {
            SyncPolicy::Group { max_delay } => {
                let shared = Arc::clone(&shared);
                Some(
                    std::thread::Builder::new()
                        .name("cutweave-journal".to_owned())
                        .spawn(move || flush_loop(&shared, max_delay))
                        .map_err(|e| JournalError::io("start journal flusher", dir, e))?,
                )
            }
            SyncPolicy::EveryCommit | SyncPolicy::Manual => None,
        };
        Ok(Journal {
            handle: Arc::new(Handle {
                shared,
                flusher: Mutex::new(flusher),
            }),
        })
    }

    /// The sink the doc actor writes committed transactions through.
    #[must_use]
    pub fn sink(&self) -> Box<dyn CommitSink> {
        Box::new(JournalSink {
            journal: self.clone(),
        })
    }

    /// Appends one committed transaction: written to the OS before this returns (and synced
    /// per the [`SyncPolicy`]). On error nothing is appended.
    pub fn append(&self, record: &CommitRecord<'_>) -> Result<(), JournalError> {
        let shared = &self.handle.shared;
        let ids = shared.ids.as_ref().map(IdTracker::state);
        let frame = match redacted_copy(shared.registry.as_deref(), record) {
            Some(commit) => {
                tracing::error!(
                    rev = %record.rev,
                    "a committed transaction reached the journal with unredacted #[secret] params; redacted them"
                );
                entry_frame(&Entry::Commit(Box::new(CommitEntry { commit, ids })))?
            }
            None => commit_frame(record, ids)?,
        };
        let now = shared.clock.now();
        let mut st = shared.lock();
        st.writable()?;
        if record.rev <= st.last_rev {
            return Err(JournalError::OutOfOrder {
                last: st.last_rev,
                rev: record.rev,
            });
        }
        let offset = write_frame(&mut st, &frame)?;
        st.seg.revs.push((record.rev, offset));
        st.last_rev = record.rev;
        st.remember(
            record.steps.iter().map(|s| s.record.clone()),
            now,
            shared.window,
        );
        made_durable(shared, &mut st)
    }

    /// Appends one change of the checkpoints (WP-2I.03): its marker is written to the OS
    /// before this returns (and synced per the [`SyncPolicy`]), and the journal keeps the
    /// checkpoint's document to carry it over the next save. On error nothing is appended.
    pub fn append_checkpoint(&self, record: &CheckpointRecord<'_>) -> Result<(), JournalError> {
        let shared = &self.handle.shared;
        let ids = shared.ids.as_ref().map(IdTracker::state);
        let frame = checkpoint_frame(record, ids)?;
        let now = shared.clock.now();
        let mut st = shared.lock();
        st.writable()?;
        if record.rev != st.last_rev {
            // Recovery rebuilds the checkpoint at the revision the marker follows.
            return Err(JournalError::OutOfOrder {
                last: st.last_rev,
                rev: record.rev,
            });
        }
        write_frame(&mut st, &frame)?;
        st.checkpoints.record(record);
        st.remember(
            record.steps.iter().map(|s| s.record.clone()),
            now,
            shared.window,
        );
        made_durable(shared, &mut st)
    }

    /// Appends one transaction marker (ADR-0043): written to the OS before this returns (and
    /// synced per the [`SyncPolicy`]). It has no revision; the journal keeps its requests and
    /// the highest id it names to carry them over the next save. On error nothing is appended.
    pub fn append_tx(&self, record: &TxMarker<'_>) -> Result<(), JournalError> {
        let shared = &self.handle.shared;
        let ids = shared.ids.as_ref().map(IdTracker::state);
        let frame = tx_frame(record, ids)?;
        let now = shared.clock.now();
        let mut st = shared.lock();
        st.writable()?;
        write_frame(&mut st, &frame)?;
        st.tx_issued = st.tx_issued.max(record.issued);
        st.remember(
            record.steps.iter().map(|s| s.record.clone()),
            now,
            shared.window,
        );
        made_durable(shared, &mut st)
    }

    /// The checkpoints the journal would carry over a save (diagnostics and tests).
    #[must_use]
    pub fn checkpoints(&self) -> CheckpointLedger {
        self.handle.shared.lock().checkpoints.clone()
    }

    /// Makes everything appended so far durable now (`fdatasync`).
    pub fn sync(&self) -> Result<(), JournalError> {
        let shared = &self.handle.shared;
        let (file, path, upto) = {
            let mut st = shared.lock();
            if st.closed {
                return Err(JournalError::Closed);
            }
            st.dirty_since = None;
            (
                Arc::clone(&st.seg.sync_file),
                st.seg.path.clone(),
                st.written,
            )
        };
        let result = file.sync_data();
        let mut st = shared.lock();
        match result {
            Ok(()) => {
                st.stats.syncs += 1;
                st.last_sync = Instant::now();
                st.synced = st.synced.max(upto);
                Ok(())
            }
            Err(e) => {
                st.error = Some(format!("fdatasync failed: {e}"));
                Err(JournalError::io("sync journal", &path, e))
            }
        }
    }

    /// Starts a new segment over the document about to be saved at committed revision
    /// `base_rev` (`doc_sha256`: SHA-256 hex of the bytes that will be written). Transactions
    /// newer than `base_rev` (committed while the save was being prepared) move to the new
    /// segment, as do the requests still inside the dedup window. Returns once the new and the
    /// old segment are durable; write the document only then, and [`retire`](Self::retire) the
    /// superseded segments after it is saved.
    ///
    /// Also clears a failure: the new segment holds every transaction since `base_rev`.
    ///
    /// Without the saved document the checkpoints made at or before `base_rev` cannot be
    /// carried over (see [`rotate_over`](Self::rotate_over)); they are lost after a restart.
    pub fn rotate(&self, base_rev: Rev, doc_sha256: &str) -> Result<Rotation, JournalError> {
        self.rotate_in(None, base_rev, doc_sha256, None)
    }

    /// [`rotate`](Self::rotate) over the document `base` that is about to be saved (as it will
    /// read back from disk): the checkpoints made at or before `base_rev` are carried into the
    /// new segment as patches from it (WP-2I.03), so they survive the save.
    pub fn rotate_over(
        &self,
        base_rev: Rev,
        doc_sha256: &str,
        base: &Project,
    ) -> Result<Rotation, JournalError> {
        self.rotate_in(None, base_rev, doc_sha256, Some(base))
    }

    /// [`rotate`](Self::rotate) into another project's journal directory (save-as), after its
    /// document is written. Hand the result to [`retire_into`](Self::retire_into).
    pub fn relocate(
        &self,
        dir: &Path,
        base_rev: Rev,
        doc_sha256: &str,
    ) -> Result<Rotation, JournalError> {
        fs::create_dir_all(dir).map_err(|e| JournalError::io("create directory", dir, e))?;
        self.rotate_in(Some(dir), base_rev, doc_sha256, None)
    }

    /// [`relocate`](Self::relocate) carrying the checkpoints, like
    /// [`rotate_over`](Self::rotate_over).
    pub fn relocate_over(
        &self,
        dir: &Path,
        base_rev: Rev,
        doc_sha256: &str,
        base: &Project,
    ) -> Result<Rotation, JournalError> {
        fs::create_dir_all(dir).map_err(|e| JournalError::io("create directory", dir, e))?;
        self.rotate_in(Some(dir), base_rev, doc_sha256, Some(base))
    }

    fn rotate_in(
        &self,
        target: Option<&Path>,
        base_rev: Rev,
        doc_sha256: &str,
        base: Option<&Project>,
    ) -> Result<Rotation, JournalError> {
        let shared = &self.handle.shared;
        // The checkpoints made before the save, as patches from the saved document: computed
        // before taking the lock, since a diff of a much-changed document takes a while and
        // commits wait for the lock. Ones made meanwhile are added under it.
        let mut patches = BTreeMap::new();
        if let Some(base) = base {
            let ledger = shared.lock().checkpoints.clone();
            carried_patches(&ledger, base_rev, base, &mut patches);
        }
        let now = shared.clock.now();
        let mut st = shared.lock();
        if st.closed {
            return Err(JournalError::Closed);
        }
        if base_rev < st.seg.header.base_rev {
            return Err(JournalError::StaleBase {
                base: st.seg.header.base_rev,
                rev: base_rev,
            });
        }
        if base_rev > st.last_rev {
            return Err(JournalError::FutureBase {
                last: st.last_rev,
                rev: base_rev,
            });
        }
        let header = SegmentHeader {
            format: FORMAT_VERSION,
            generation: st.next_generation,
            base_rev,
            doc_sha256: doc_sha256.to_owned(),
            created_at: now,
            seed: shared.ids.as_ref().map(IdTracker::seed),
            ids: shared.ids.as_ref().map(IdTracker::state),
        };
        st.remember(std::iter::empty(), now, shared.window);
        let carry = carry_over(&st, base_rev, base, &mut patches);
        let mut bytes = segment_prefix(&header, carry)?;
        let prefix = bytes.len() as u64;
        let first = st.seg.revs.partition_point(|(rev, _)| *rev <= base_rev);
        let mut revs = Vec::new();
        if let Some(&(_, from)) = st.seg.revs.get(first) {
            let tail = read_range(&st.seg.path, from, st.seg.len)
                .map_err(|e| JournalError::io("read journal", &st.seg.path, e))?;
            revs.extend(
                st.seg.revs[first..]
                    .iter()
                    .map(|&(rev, off)| (rev, off - from + prefix)),
            );
            bytes.extend(tail);
        }
        let dir = target.map_or_else(|| st.dir.clone(), Path::to_path_buf);
        let path = segment_path(&dir, base_rev);
        let (mut file, sync_file) = create_segment(&path, &bytes)?;
        file.seek(SeekFrom::End(0))
            .map_err(|e| JournalError::io("seek journal", &path, e))?;
        st.dir = dir;
        let new_sync = Arc::new(sync_file);
        let old = std::mem::replace(
            &mut st.seg,
            Segment {
                path,
                file,
                sync_file: Arc::clone(&new_sync),
                header,
                len: bytes.len() as u64,
                revs,
            },
        );
        st.next_generation += 1;
        st.chain.push(old.path.clone());
        let superseded = st.chain.clone();
        st.error = None;
        st.dirty_since = None;
        st.stats.rotations += 1;
        let (new_path, dir, upto) = (st.seg.path.clone(), st.dir.clone(), st.written);
        drop(st);

        // Durable before the document is written: the new segment (its header names the
        // document) and the old one (the chain from the previous document continues through
        // it if the document write fails or is cut short).
        new_sync
            .sync_data()
            .map_err(|e| JournalError::io("sync journal", &new_path, e))?;
        old.sync_file
            .sync_data()
            .map_err(|e| JournalError::io("sync journal", &old.path, e))?;
        sync_dir(&dir);
        let mut st = shared.lock();
        st.stats.syncs += 2;
        st.last_sync = Instant::now();
        st.synced = st.synced.max(upto);
        Ok(Rotation { superseded })
    }

    /// After the document of a [`rotate`](Self::rotate) is saved: the segment it superseded
    /// is kept until the next successful save (the "previous journal"); older ones are
    /// deleted.
    pub fn retire(&self, rotation: Rotation) {
        let mut superseded = rotation.superseded;
        let keep = superseded.pop();
        let mut st = self.handle.shared.lock();
        st.chain
            .retain(|p| !superseded.contains(p) && Some(p) != keep.as_ref());
        let old_retained = std::mem::replace(&mut st.retained, keep.into_iter().collect());
        drop(st);
        for path in superseded.iter().chain(&old_retained) {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "could not delete an old journal segment");
                }
            }
        }
    }

    /// After a [`relocate`](Self::relocate): moves every segment of the old directory (the
    /// superseded chain and the previous journal) into `backups` (the old project's backups
    /// folder), so the old project opens at its last save without offering the moved work.
    pub fn retire_into(&self, rotation: Rotation, backups: &Path) {
        let mut st = self.handle.shared.lock();
        st.chain.retain(|p| !rotation.superseded.contains(p));
        let retained = std::mem::take(&mut st.retained);
        let now = shared_now(&self.handle.shared);
        drop(st);
        if let Err(e) = fs::create_dir_all(backups) {
            tracing::warn!(path = %backups.display(), error = %e, "could not create the backups folder");
        }
        for path in rotation.superseded.into_iter().chain(retained) {
            let name = path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let target = backups.join(format!("journal-saved-as-{}-{name}", now.as_millis()));
            if let Err(e) = fs::rename(&path, &target) {
                tracing::warn!(path = %path.display(), error = %e, "could not move an old journal segment");
            }
        }
    }

    /// Syncs, stops the flusher thread and refuses further commits. Also happens when the
    /// last handle is dropped; use this to observe errors.
    pub fn close(&self) -> Result<(), JournalError> {
        self.handle.shutdown()
    }

    /// The committed revision of the last journaled transaction (or the base revision).
    #[must_use]
    pub fn last_rev(&self) -> Rev {
        self.handle.shared.lock().last_rev
    }

    /// The committed revision the current segment builds on (the last save).
    #[must_use]
    pub fn base_rev(&self) -> Rev {
        self.handle.shared.lock().seg.header.base_rev
    }

    /// The current segment file.
    #[must_use]
    pub fn segment_path(&self) -> PathBuf {
        self.handle.shared.lock().seg.path.clone()
    }

    /// The durability policy.
    #[must_use]
    pub fn sync_policy(&self) -> SyncPolicy {
        self.handle.shared.sync
    }

    /// Whether everything appended has been synced.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        let st = self.handle.shared.lock();
        st.synced == st.written
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> JournalStats {
        self.handle.shared.lock().stats
    }

    /// The failure that makes the journal refuse commits, if any.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        self.handle.shared.lock().error.clone()
    }
}

fn shared_now(shared: &Shared) -> WallTime {
    shared.clock.now()
}

/// Appends `frame` to the current segment; returns its offset. A failed write is cut off again
/// so later entries stay reachable (if that fails too, the journal refuses further writes).
fn write_frame(st: &mut State, frame: &[u8]) -> Result<u64, JournalError> {
    let offset = st.seg.len;
    if let Err(e) = st.seg.file.write_all(frame) {
        let undo = st
            .seg
            .file
            .set_len(offset)
            .and_then(|()| st.seg.file.seek(SeekFrom::Start(offset)).map(|_| ()));
        if let Err(u) = undo {
            st.error = Some(format!("write failed ({e}) and could not be undone ({u})"));
        }
        return Err(JournalError::io("append to journal", &st.seg.path, e));
    }
    st.seg.len += frame.len() as u64;
    st.written += 1;
    st.stats.appends += 1;
    st.stats.bytes += frame.len() as u64;
    Ok(offset)
}

/// Applies the [`SyncPolicy`] after an append.
fn made_durable(shared: &Shared, st: &mut State) -> Result<(), JournalError> {
    match shared.sync {
        SyncPolicy::EveryCommit => {
            if let Err(e) = st.seg.file.sync_data() {
                st.error = Some(format!("fdatasync failed: {e}"));
                return Err(JournalError::io("sync journal", &st.seg.path, e));
            }
            st.stats.syncs += 1;
            st.last_sync = Instant::now();
            st.synced = st.written;
        }
        SyncPolicy::Group { .. } => {
            if st.dirty_since.is_none() {
                st.dirty_since = Some(Instant::now());
                shared.wake.notify_one();
            }
        }
        SyncPolicy::Manual => {
            st.dirty_since.get_or_insert_with(Instant::now);
        }
    }
    Ok(())
}

/// A copy of `record` with `#[secret]` params redacted, if any step still carries one.
fn redacted_copy(
    registry: Option<&Registry>,
    record: &CommitRecord<'_>,
) -> Option<JournaledCommit> {
    let registry = registry?;
    let leaks = record.steps.iter().any(|step| {
        registry.get(&step.command).is_some_and(|e| {
            !e.meta.secret_fields.is_empty() && e.redact(&step.params) != step.params
        })
    });
    if !leaks {
        return None;
    }
    let mut commit = record.to_owned();
    for step in &mut commit.steps {
        if let Some(entry) = registry.get(&step.command) {
            step.params = entry.redact(&step.params);
        }
    }
    Some(commit)
}

/// Group commit: `fdatasync` within `max_delay` of the first unsynced write.
fn flush_loop(shared: &Shared, max_delay: Duration) {
    let mut st = shared.lock();
    loop {
        if st.closed {
            return;
        }
        let Some(_) = st.dirty_since else {
            st = shared.wake.wait(st).unwrap_or_else(PoisonError::into_inner);
            continue;
        };
        let due = st.last_sync + max_delay;
        let now = Instant::now();
        if now < due {
            st = shared
                .wake
                .wait_timeout(st, due - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            continue;
        }
        st.dirty_since = None;
        let (file, path, upto) = (
            Arc::clone(&st.seg.sync_file),
            st.seg.path.clone(),
            st.written,
        );
        drop(st);
        let result = file.sync_data();
        st = shared.lock();
        st.last_sync = Instant::now();
        match result {
            Ok(()) => {
                st.stats.syncs += 1;
                st.synced = st.synced.max(upto);
            }
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "journal fdatasync failed; refusing further commits until the next save");
                st.error = Some(format!("fdatasync failed: {e}"));
            }
        }
    }
}

/// The doc actor's [`CommitSink`].
struct JournalSink {
    journal: Journal,
}

impl CommitSink for JournalSink {
    fn commit(&mut self, record: &CommitRecord<'_>) -> Result<(), CmdError> {
        self.journal.append(record).map_err(|e| {
            tracing::error!(rev = %record.rev, error = %e, "journal write failed; the transaction was not applied");
            CmdError::internal(format!(
                "the change could not be journaled and was not applied: {e}"
            ))
        })
    }

    fn tx_marker(&mut self, record: &TxMarker<'_>) -> Result<(), CmdError> {
        self.journal.append_tx(record).map_err(|e| {
            tracing::error!(issued = record.issued, error = %e, "journal write failed; the transaction marker was not written");
            CmdError::internal(format!(
                "the journal could not be written: {e}"
            ))
        })
    }

    fn checkpoint(&mut self, record: &CheckpointRecord<'_>) -> Result<(), CmdError> {
        self.journal.append_checkpoint(record).map_err(|e| {
            tracing::error!(rev = %record.rev, error = %e, "journal write failed; the checkpoint change was not applied");
            CmdError::internal(format!(
                "the checkpoint change could not be journaled and was not applied: {e}"
            ))
        })
    }
}
