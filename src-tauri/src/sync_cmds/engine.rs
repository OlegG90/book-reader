//! The sync engine: one worker thread, the only thing that talks to the
//! transports in the background. Windows report what happened (a position
//! moved, a book closed, a download was asked for) by queueing jobs in the
//! outbox; the worker runs them, retries failures per job with a growing
//! wait, holds book files on metered networks, and keeps the outbox in the
//! store so nothing is lost on a restart. It also runs the startup pull, once
//! per start of the app.

use super::{book_transfer_allowed, enabled, pull_all_clouds, pull_book_from_cloud, push_book_to_cloud, transports, BookKey, Transport};
use crate::clock::now_ms;
use crate::library_cmds::LibraryBooks;
use crate::store::{Job, Pending, Store};
use crate::sync_error::Kind;
use std::sync::{Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// How long the worker sleeps with nothing due: it also looks again whether
/// an unmetered network is back for held book files.
const IDLE_MS: u64 = 60_000;
/// How long a book file waits before the network is checked again.
const HOLD_MS: u64 = 60_000;
/// The first retry's wait; each failure doubles it, up to `BACKOFF_MAX_MS`.
const BACKOFF_FIRST_MS: u64 = 30_000;
const BACKOFF_MAX_MS: u64 = 30 * 60_000;
/// A server that asks to slow down waits at least this long.
const RATE_LIMIT_MS: u64 = 5 * 60_000;

/// Adds a job to the outbox; the same work already waiting is replaced, so
/// only a book's newest position per transport is kept, and the new job is
/// due at once.
pub(crate) fn queue(outbox: &mut Vec<Pending>, job: Job) {
    outbox.retain(|p| !p.job.same_work(&job));
    outbox.push(Pending { job, attempts: 0, not_before: 0 });
}

/// What one try of a job came to.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// Done, or nothing left to do (the transport is off or has no login).
    Done,
    /// Waits for an unmetered network; not a failure.
    Hold,
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

/// Tries one job; a book that arrives from a cloud is added to `arrived`.
fn work(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], job: &Job, arrived: &mut Vec<String>) -> Verdict {
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
        Job::Upload { .. } | Job::Download { .. } if !book_transfer_allowed(store) => Verdict::Hold,
        Job::Upload { fingerprint } => match push_book_to_cloud(store, books, transports, fingerprint) {
            Ok(_) => Verdict::Done,
            Err(_) => Verdict::Retry(Kind::Other),
        },
        Job::Download { fingerprint } => match pull_book_from_cloud(store, transports, fingerprint) {
            Ok(landed) => {
                if landed {
                    arrived.push(fingerprint.clone());
                }
                Verdict::Done
            }
            Err(_) => Verdict::Retry(Kind::Other),
        },
    }
}

/// Records how a try went. Only the job as it was tried is touched: a newer
/// one queued meanwhile for the same work stays as it was queued.
fn settle(outbox: &mut Vec<Pending>, tried: &Pending, verdict: Verdict, now: u64) {
    let Some(i) = outbox.iter().position(|p| p.job == tried.job) else { return };
    match verdict {
        Verdict::Done => {
            outbox.remove(i);
        }
        Verdict::Hold => outbox[i].not_before = now + HOLD_MS,
        Verdict::Retry(kind) => {
            outbox[i].attempts += 1;
            outbox[i].not_before = now + backoff(outbox[i].attempts, kind);
        }
    }
}

/// Runs every job that is due at `now`, once; returns the fingerprints of
/// books that arrived from a cloud.
pub(super) fn run_due(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], now: u64) -> Vec<String> {
    let due: Vec<Pending> = store.read(|s| s.outbox.iter().filter(|p| p.not_before <= now).cloned().collect());
    let mut arrived = Vec::new();
    for tried in due {
        let verdict = work(store, books, transports, &tried.job, &mut arrived);
        let _ = store.update(|s| settle(&mut s.outbox, &tried, verdict, now));
    }
    arrived
}

/// How long the worker may sleep: until the next job is due, at most `IDLE_MS`.
fn sleep_for(store: &Store, now: u64) -> u64 {
    let next = store.read(|s| s.outbox.iter().map(|p| p.not_before).min());
    next.map_or(IDLE_MS, |due| due.saturating_sub(now)).clamp(1_000, IDLE_MS)
}

/// Wakes the worker early: new work was queued, or the app came back to the front.
#[derive(Default)]
pub(crate) struct SyncEngine {
    woken: Mutex<bool>,
    bell: Condvar,
}

impl SyncEngine {
    /// Tells the worker to look at the outbox now.
    pub(crate) fn wake(&self) {
        *self.woken.lock().unwrap() = true;
        self.bell.notify_one();
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
            let _ = app.emit("sync-positions-updated", ());
        }
        loop {
            if !run_due(&store, &books, &transports(), now_ms()).is_empty() {
                let _ = app.emit("sync-books-arrived", ());
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
        let tried = outbox[0].clone();
        settle(&mut outbox, &tried, Verdict::Hold, 2_000);
        assert_eq!((outbox[0].attempts, outbox[0].not_before), (1, 2_000 + HOLD_MS), "a hold is no failure");
    }
}
