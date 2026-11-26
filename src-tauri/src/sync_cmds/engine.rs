//! The sync engine: one worker thread, the only thing that talks to the
//! transports in the background. Windows report what happened (a position
//! moved, a book closed, a download was asked for) by queueing jobs in the
//! outbox; the worker runs them, retries failures per job with a growing
//! wait, holds book files on metered networks, and keeps the outbox in the
//! store so nothing is lost on a restart. It also runs the startup pull, once
//! per start of the app.

use super::{book_stores, book_transfer_allowed, cloud_books, enabled, latest_of, pull_all_clouds, pull_book_from_cloud, push_book_to_cloud, transports, BookKey, Transport};
use crate::clock::now_ms;
use crate::library_cmds::{library_root, LibraryBooks};
use crate::platform::{Active, Platform};
use crate::cloud::BookRecord;
use crate::library::Entry;
use crate::store::{Job, Pending, Store};
use std::collections::{BTreeSet, HashMap};
use crate::sync_error::Kind;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// How long the worker sleeps with nothing due: it also looks again whether
/// an unmetered network is back for held book files.
const IDLE_MS: u64 = 60_000;
/// The first retry's wait; each failure doubles it, up to `BACKOFF_MAX_MS`.
const BACKOFF_FIRST_MS: u64 = 30_000;
const BACKOFF_MAX_MS: u64 = 30 * 60_000;
/// A server that asks to slow down waits at least this long.
const RATE_LIMIT_MS: u64 = 5 * 60_000;
/// How often the library is compared with the clouds with nothing asking:
/// another device may have added or removed books meanwhile.
const RECONCILE_MS: u64 = 10 * 60_000;
/// Up to this much the engine downloads new cloud books by itself; more
/// waits for the reader's yes (the library's download offer).
const AUTO_BOOKS: usize = 10;
const AUTO_BYTES: u64 = 100 * 1024 * 1024;

/// A job that failed this often is dropped: what still fails after hours of
/// tries (a broken file, a library gone, a wrong password) won't heal by
/// itself. Queueing the same work again starts over.
const MAX_ATTEMPTS: u32 = 12;

/// Adds a job to the outbox; the same work already waiting is replaced, so
/// only a book's newest position per transport is kept, and the new job is
/// due at once.
pub(crate) fn queue(outbox: &mut Vec<Pending>, job: Job) {
    outbox.retain(|p| !p.job.same_work(&job));
    outbox.push(Pending { job, attempts: 0, not_before: 0, no_room: false });
}

/// Adds a job unless the same work is already waiting: the reconcile finds
/// the same work every round, and must not wipe its wait after failures.
fn queue_once(outbox: &mut Vec<Pending>, job: Job) {
    if !outbox.iter().any(|p| p.job.same_work(&job)) {
        outbox.push(Pending { job, attempts: 0, not_before: 0, no_room: false });
    }
}

/// What one try of a job came to.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// Done, or nothing left to do (the transport is off or has no login).
    Done,
    /// Failed; tried again later, the wait set by the failure's kind.
    Retry(Kind),
}

/// How long a job waits after its `attempts`-th failure.
fn backoff(attempts: u32, kind: Kind) -> u64 {
    let wait = BACKOFF_FIRST_MS.saturating_mul(1 << attempts.clamp(1, 10).saturating_sub(1)).min(BACKOFF_MAX_MS);
    if kind == Kind::RateLimited {
        wait.max(RATE_LIMIT_MS)
    } else {
        wait
    }
}

/// Whether a job moves a book file (held on metered networks).
fn moves_a_file(job: &Job) -> bool {
    matches!(job, Job::Upload { .. } | Job::Download { .. })
}

/// Removes a book from every cloud that holds it; done when none does. A
/// connected cloud that can't be read counts as failed, so the removal
/// reaches it later. Until then the book stays in Removed books, so it is
/// never downloaded back meanwhile.
fn remove_everywhere(store: &Store, transports: &[&dyn Transport], fingerprint: &str) -> Verdict {
    let mut failed = None;
    for t in book_stores(store, transports) {
        let Some(books) = t.books() else {
            failed = Some(Kind::Offline);
            continue;
        };
        let Some(record) = books.get(fingerprint).filter(|r| r.removed.is_none()) else { continue };
        if let Err(e) = t.remove_book(store, fingerprint, record) {
            failed = Some(e.kind);
        }
    }
    if failed.is_none() {
        let _ = store.update(|s| {
            s.removed_here.remove(fingerprint);
        });
    }
    failed.map_or(Verdict::Done, Verdict::Retry)
}

/// Whether a job may run at `now`: it is due, and a book file only when the
/// network allows book files. A held file waits without a write or a retry.
fn runnable(pending: &Pending, now: u64, files_allowed: bool) -> bool {
    pending.not_before <= now && (files_allowed || !moves_a_file(&pending.job))
}

/// Whether book files may move now; the network is asked (a bridge call on
/// the phone) only when a file job among those `pick` chooses is waiting.
/// The engine owns this gate: the transfers themselves don't check it.
fn files_allowed(store: &Store, pick: impl Fn(&Job) -> bool) -> bool {
    store.read(|s| s.outbox.iter().any(|p| moves_a_file(&p.job) && pick(&p.job))) && book_transfer_allowed(store)
}

/// What a run of the outbox brought: books that arrived from a cloud, and
/// books now uploaded.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Outcome {
    pub(super) arrived: Vec<String>,
    pub(super) uploaded: Vec<String>,
}

/// Tries one job, noting in `outcome` what it brought.
fn work(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], job: &Job, outcome: &mut Outcome) -> Verdict {
    match job {
        Job::Position { transport, fingerprint, document, cfi, fraction } => {
            let Some(t) = transports.iter().find(|t| t.id() == transport) else { return Verdict::Done };
            if !enabled(store, *t) {
                return Verdict::Done;
            }
            match t.push(store, &BookKey { document, fingerprint }, cfi, *fraction) {
                Ok(()) => Verdict::Done,
                Err(e) if e.kind == Kind::NotConnected => Verdict::Done,
                Err(e) => Verdict::Retry(e.kind),
            }
        }
        Job::Upload { fingerprint } => match push_book_to_cloud(store, books, transports, fingerprint) {
            Ok(_) => {
                outcome.uploaded.push(fingerprint.clone());
                Verdict::Done
            }
            Err(e) => Verdict::Retry(e.kind),
        },
        Job::Download { fingerprint } => match pull_book_from_cloud(store, transports, fingerprint) {
            Ok(landed) => {
                if landed {
                    outcome.arrived.push(fingerprint.clone());
                }
                Verdict::Done
            }
            Err(_) => Verdict::Retry(Kind::Other),
        },
        Job::Remove { fingerprint } => remove_everywhere(store, transports, fingerprint),
    }
}

/// What a reconcile changed in the library, and learned of the clouds.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Reconciled {
    /// Library books removed from the clouds on another device, moved out here.
    pub(super) moved_out: usize,
    /// Library books every readable cloud holds.
    pub(super) in_clouds: BTreeSet<String>,
    /// Each cloud's used and total space, by transport id.
    pub(super) space: HashMap<String, (u64, u64)>,
}

/// What each library book needs: an upload (a cloud lacks it, or it came
/// back after its removal), or to leave this library (another device removed
/// it from the clouds after it came here). Returns the uploads' fingerprints
/// and the books to move out.
/// A book found by the library's first scan carries its file's date, which
/// says nothing about when it came to this device; for removals it counts as
/// arriving at that first scan (`since`), so a copied-in old collection never
/// loses books removed before it arrived.
pub(super) fn sort_library(
    library: &[Entry],
    per_cloud: &[HashMap<String, BookRecord>],
    latest: &HashMap<String, BookRecord>,
    since: u64,
) -> (Vec<String>, Vec<Entry>) {
    let mut uploads = Vec::new();
    let mut moved_out = Vec::new();
    for entry in library {
        let arrived = entry.added.max(since);
        match latest.get(&entry.fingerprint) {
            Some(record) if record.removed.is_some_and(|at| at >= arrived) => moved_out.push(entry.clone()),
            _ if per_cloud.iter().any(|books| books.get(&entry.fingerprint).is_none_or(|r| r.removed.is_some())) => {
                uploads.push(entry.fingerprint.clone());
            }
            _ => {}
        }
    }
    (uploads, moved_out)
}

/// Compares the library with the clouds (see Library sync in the spec):
/// - books the clouds lack (or a book added again after its removal) are
///   queued for upload;
/// - new cloud books are queued for download when there are few of them (more
///   wait for the reader's yes); books removed here on purpose never are;
/// - books another device removed from the clouds after they came here go
///   the way Delete from library does (the Recycle Bin on Windows);
/// - Removed books forgets books the clouds no longer hold.
pub(super) fn reconcile(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport]) -> Reconciled {
    let Ok(root) = library_root(store) else { return Reconciled::default() };
    let per_cloud = cloud_books(store, transports);
    let Some(latest) = latest_of(&per_cloud) else { return Reconciled::default() };
    let library = books.snapshot(store, &root);
    let removed_here = store.read(|s| s.removed_here.clone());
    let since = store.read(|s| s.library_since.as_ref().filter(|(at, _)| *at == root).map_or(0, |(_, t)| *t));
    let (mut uploads, moved_out) = sort_library(&library, &per_cloud, &latest, since);
    // Books being read go up first, then the newest added.
    let positions = store.read(|s| s.positions.clone());
    let added: HashMap<&str, u64> = library.iter().map(|e| (e.fingerprint.as_str(), e.added)).collect();
    let reading = |f: &String| positions.get(f).is_some_and(|p| p.fraction > 0.0 && p.fraction < 0.98);
    uploads.sort_by_key(|f| (!reading(f), std::cmp::Reverse(added.get(f.as_str()).copied().unwrap_or(0))));
    let mut jobs: Vec<Job> = uploads.into_iter().map(|fingerprint| Job::Upload { fingerprint }).collect();
    let in_clouds: BTreeSet<String> = library
        .iter()
        .filter(|e| per_cloud.iter().all(|books| books.get(&e.fingerprint).is_some_and(|r| r.removed.is_none())))
        .map(|e| e.fingerprint.clone())
        .collect();
    let space: HashMap<String, (u64, u64)> = book_stores(store, transports).iter().filter_map(|t| Some((t.id().to_string(), t.space()?))).collect();
    let local: BTreeSet<String> = library.iter().map(|b| b.fingerprint.clone()).collect();
    let missing = super::collect_missing(store, &latest, &local, &removed_here);
    if missing.len() <= AUTO_BOOKS && missing.iter().map(|m| m.size).sum::<u64>() <= AUTO_BYTES {
        jobs.extend(missing.into_iter().map(|m| Job::Download { fingerprint: m.fingerprint }));
    }
    let _ = store.update(|s| {
        for job in jobs {
            queue_once(&mut s.outbox, job);
        }
        // Removed books forgets only books removed from the clouds: a cloud
        // that couldn't be read, or a book not uploaded yet, keeps its entry.
        s.removed_here.retain(|fingerprint| latest.get(fingerprint).is_none_or(|r| r.removed.is_none()));
    });
    let mut moved = 0;
    for entry in moved_out {
        if Active::delete_book(&entry.absolute(&root)).is_ok() {
            books.forget(&entry.path);
            moved += 1;
        }
    }
    Reconciled { moved_out: moved, in_clouds, space }
}

/// Records how a try went. Only the job as it was tried is touched: a newer
/// one queued meanwhile for the same work stays as it was queued.
fn settle(outbox: &mut Vec<Pending>, tried: &Pending, verdict: Verdict, now: u64) {
    let Some(i) = outbox.iter().position(|p| p.job == tried.job) else { return };
    match verdict {
        Verdict::Done => {
            outbox.remove(i);
        }
        // A full cloud is no fault of the book: it waits the longest wait,
        // without counting towards giving up, until space frees.
        Verdict::Retry(Kind::NoSpace) => {
            outbox[i].no_room = true;
            outbox[i].not_before = now + BACKOFF_MAX_MS;
        }
        Verdict::Retry(kind) => {
            outbox[i].no_room = false;
            outbox[i].attempts += 1;
            if outbox[i].attempts >= MAX_ATTEMPTS {
                outbox.remove(i);
            } else {
                outbox[i].not_before = now + backoff(outbox[i].attempts, kind);
            }
        }
    }
}

/// Runs every job that may run at `now` and that `pick` chooses, once, and
/// records the outcomes in one write; says what the run brought. A job that panics counts as failed, so one bad
/// job never stops the engine.
fn run(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], now: u64, pick: impl Fn(&Job) -> bool) -> Outcome {
    let files_allowed = files_allowed(store, &pick);
    let due: Vec<Pending> =
        store.read(|s| s.outbox.iter().filter(|p| runnable(p, now, files_allowed) && pick(&p.job)).cloned().collect());
    if due.is_empty() {
        return Outcome::default();
    }
    let mut outcome = Outcome::default();
    let mut verdicts = Vec::new();
    for tried in due {
        let verdict = catch_unwind(AssertUnwindSafe(|| work(store, books, transports, &tried.job, &mut outcome)))
            .unwrap_or(Verdict::Retry(Kind::Other));
        verdicts.push((tried, verdict));
    }
    let _ = store.update(|s| {
        for (tried, verdict) in verdicts {
            settle(&mut s.outbox, &tried, verdict, now);
        }
    });
    outcome
}

/// Runs every job that may run at `now`; says what it brought.
pub(super) fn run_due(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], now: u64) -> Outcome {
    run(store, books, transports, now, |_| true)
}

/// Sends the positions that are due now, in the caller's thread: a closing
/// window waits for them, as the worker dies with the app.
pub(super) fn run_positions(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], now: u64) {
    run(store, books, transports, now, |job| matches!(job, Job::Position { .. }));
}

/// How long the worker may sleep: until the next job that can run is due,
/// at most `IDLE_MS`.
fn sleep_for(store: &Store, now: u64) -> u64 {
    let files_allowed = files_allowed(store, |_| true);
    let next = store.read(|s| s.outbox.iter().filter(|p| files_allowed || !moves_a_file(&p.job)).map(|p| p.not_before).min());
    next.map_or(IDLE_MS, |due| due.saturating_sub(now)).clamp(1_000, IDLE_MS)
}

/// What the last reconcile learned of the clouds: the library books every
/// cloud holds, and each cloud's used and total space by transport id.
type Known = (BTreeSet<String>, HashMap<String, (u64, u64)>);

/// The worker's doorbell, and what the windows ask it.
#[derive(Default)]
pub(crate) struct SyncEngine {
    woken: Mutex<bool>,
    bell: Condvar,
    /// The library changed (a scan, a delete): compare it with the clouds
    /// on the next round.
    reconcile: AtomicBool,
    /// What the last reconcile learned.
    known: Mutex<Known>,
    /// The startup pull changed positions (for a window that started
    /// listening after it said so).
    pulled: AtomicBool,
}

impl SyncEngine {
    /// Tells the worker to look at the outbox now.
    pub(crate) fn wake(&self) {
        *self.woken.lock().unwrap() = true;
        self.bell.notify_one();
    }

    /// Asks for the library to be compared with the clouds, and wakes the worker.
    pub(crate) fn request_reconcile(&self) {
        self.reconcile.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// The library books every readable cloud holds, as last learned.
    pub(crate) fn in_clouds(&self) -> BTreeSet<String> {
        self.known.lock().unwrap().0.clone()
    }

    /// A cloud's used and total space, as last learned.
    pub(crate) fn space(&self, cloud: &str) -> Option<(u64, u64)> {
        self.known.lock().unwrap().1.get(cloud).copied()
    }

    /// Whether the startup pull changed positions.
    pub(crate) fn pulled(&self) -> bool {
        self.pulled.load(Ordering::SeqCst)
    }

    /// Sleeps up to `ms`, or until woken.
    fn sleep(&self, ms: u64) {
        let woken = self.woken.lock().unwrap();
        let (mut woken, _) = self.bell.wait_timeout_while(woken, Duration::from_millis(ms), |w| !*w).unwrap();
        *woken = false;
    }
}

/// Starts the worker; the store must be managed already. The windows hear
/// `sync-positions-updated` after the startup pull changed positions, and
/// `sync-books-arrived` when downloaded books land in the library.
pub(crate) fn start(app: &AppHandle) {
    app.manage(SyncEngine::default());
    let app = app.clone();
    std::thread::spawn(move || {
        let store = app.state::<Store>();
        let books = app.state::<LibraryBooks>();
        let engine = app.state::<SyncEngine>();
        // Silent when offline: a failed start is the normal no-network case.
        if pull_all_clouds(&store, &books, &transports()).is_ok_and(|updated| updated > 0) {
            engine.pulled.store(true, Ordering::SeqCst);
            let _ = app.emit("sync-positions-updated", ());
        }
        let mut reconciled_at = 0;
        loop {
            let now = now_ms();
            if engine.reconcile.swap(false, Ordering::SeqCst) || now.saturating_sub(reconciled_at) >= RECONCILE_MS {
                reconciled_at = now;
                let found = reconcile(&store, &books, &transports());
                if found.moved_out > 0 {
                    let _ = app.emit("library-updated", ());
                }
                *engine.known.lock().unwrap() = (found.in_clouds, found.space);
                let _ = app.emit("sync-states-changed", ());
            }
            let ran = run_due(&store, &books, &transports(), now_ms());
            if !ran.arrived.is_empty() {
                let _ = app.emit("sync-books-arrived", ());
            }
            if !ran.uploaded.is_empty() {
                engine.known.lock().unwrap().0.extend(ran.uploaded);
                let _ = app.emit("sync-states-changed", ());
            }
            engine.sleep(sleep_for(&store, now_ms()));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(transport: &str, cfi: &str) -> Job {
        Job::Position { transport: transport.into(), fingerprint: "fp".into(), document: "doc".into(), cfi: cfi.into(), fraction: 0.5 }
    }

    #[test]
    fn a_newer_position_replaces_the_waiting_one_per_transport() {
        let mut outbox = Vec::new();
        queue(&mut outbox, position("kosync", "a"));
        queue(&mut outbox, position("dropbox", "a"));
        outbox[0].attempts = 3;
        queue(&mut outbox, position("kosync", "b"));
        assert_eq!(outbox.len(), 2);
        let kosync = outbox.iter().find(|p| matches!(&p.job, Job::Position { transport, .. } if transport == "kosync")).unwrap();
        assert_eq!((kosync.job.clone(), kosync.attempts, kosync.not_before), (position("kosync", "b"), 0, 0));
        queue(&mut outbox, Job::Upload { fingerprint: "fp".into() });
        queue(&mut outbox, Job::Upload { fingerprint: "fp".into() });
        assert_eq!(outbox.len(), 3, "one upload per book");
    }

    #[test]
    fn failures_wait_longer_each_time_and_rate_limits_longer_still() {
        assert_eq!(backoff(1, Kind::Offline), 30_000);
        assert_eq!(backoff(2, Kind::Offline), 60_000);
        assert_eq!(backoff(3, Kind::Http), 120_000);
        assert_eq!(backoff(40, Kind::Offline), BACKOFF_MAX_MS);
        assert_eq!(backoff(1, Kind::RateLimited), RATE_LIMIT_MS);
    }

    #[test]
    fn a_try_settles_only_the_job_it_tried() {
        let mut outbox = Vec::new();
        queue(&mut outbox, position("kosync", "a"));
        let tried = outbox[0].clone();
        // A newer position came in while the old one was on its way.
        queue(&mut outbox, position("kosync", "b"));
        settle(&mut outbox, &tried, Verdict::Done, 1_000);
        assert_eq!(outbox.len(), 1, "the newer one still goes");
        let tried = outbox[0].clone();
        settle(&mut outbox, &tried, Verdict::Retry(Kind::Offline), 1_000);
        assert_eq!((outbox[0].attempts, outbox[0].not_before), (1, 31_000));
    }

    #[test]
    fn a_full_cloud_keeps_the_book_waiting_without_giving_up() {
        let mut outbox = Vec::new();
        queue(&mut outbox, Job::Upload { fingerprint: "fp".into() });
        for _ in 0..MAX_ATTEMPTS + 2 {
            let tried = outbox[0].clone();
            settle(&mut outbox, &tried, Verdict::Retry(Kind::NoSpace), 0);
        }
        assert_eq!((outbox.len(), outbox[0].no_room, outbox[0].not_before), (1, true, BACKOFF_MAX_MS));
        let tried = outbox[0].clone();
        settle(&mut outbox, &tried, Verdict::Retry(Kind::Offline), 0);
        assert!(!outbox[0].no_room, "another failure isn't a full cloud");
    }

    #[test]
    fn a_job_that_keeps_failing_is_dropped() {
        let mut outbox = Vec::new();
        queue(&mut outbox, Job::Download { fingerprint: "fp".into() });
        for _ in 0..MAX_ATTEMPTS {
            let tried = outbox[0].clone();
            settle(&mut outbox, &tried, Verdict::Retry(Kind::Other), 0);
        }
        assert!(outbox.is_empty());
    }

    #[test]
    fn book_files_wait_for_an_allowed_network_without_counting_a_failure() {
        let upload = Pending { job: Job::Upload { fingerprint: "fp".into() }, attempts: 0, not_before: 0, no_room: false };
        let push = Pending { job: position("kosync", "a"), attempts: 0, not_before: 0, no_room: false };
        assert!(!runnable(&upload, 10, false));
        assert!(runnable(&push, 10, false), "positions always go");
        assert!(runnable(&upload, 10, true));
        assert!(!runnable(&Pending { not_before: 20, ..push }, 10, true), "not due yet");
    }
}
