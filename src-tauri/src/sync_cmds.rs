//! Reading progress sync (KOSync): what the server says about a book.

use crate::catalog;
use crate::clock::now_ms;
use crate::cloud::RemoteRecord;
use crate::kosync;
use crate::modules;
use crate::store::{self, Position, Store};
use crate::windows::ReaderWindows;
use crate::CmdResult;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use tauri::{Manager, State, Window};

/// The source that syncs progress, if one is set and sync is on: with the
/// module off the sync commands no-op (no network) instead of failing.
fn sync_source(store: &Store) -> Option<catalog::Source> {
    if store.read(|s| modules::is_off(s, "capability:progress-sync")) {
        return None;
    }
    store.read(|s| s.sources.iter().find(|source| source.sync).cloned())
}

/// What a transport reads a book by: KOSync's document id, the cloud's fingerprint.
struct BookKey<'a> {
    document: &'a str,
    fingerprint: &'a str,
}

/// One way to carry positions between devices. KOSync today; a cloud store
/// next. Every transport speaks `Progress` and names itself, so pulling fans
/// out and pushing reports per transport without learning transports.
trait Transport {
    /// "kosync", "dropbox", … — the Modules reason and the frontend's pending set name it.
    fn id(&self) -> &'static str;
    /// False when the transport's module is off.
    fn enabled(&self, state: &store::State) -> bool;
    /// The newest remote position for the book, if any.
    fn pull(&self, store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, String>;
    /// Sends the position.
    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), String>;
    /// The cloud's whole positions file, for the startup pull; transports
    /// without one (KOSync) have nothing to fetch in bulk.
    fn remote(&self, _store: &Store) -> Option<HashMap<String, RemoteRecord>> {
        None
    }
}

/// Progress sync through the catalog server (see kosync.rs).
struct Kosync;

impl Transport for Kosync {
    fn id(&self) -> &'static str {
        "kosync"
    }

    fn enabled(&self, state: &store::State) -> bool {
        // KOSync works through the catalog server (its own transport rule,
        // not a module edge, so clouds survive catalogs being turned off),
        // and needs a source marked for sync.
        !modules::is_off(state, "capability:progress-sync")
            && !modules::is_off(state, "capability:catalogs")
            && state.sources.iter().any(|s| s.sync)
    }

    fn pull(&self, store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, String> {
        let Some(source) = sync_source(store) else { return Ok(None) };
        kosync::pull(&source, book.document)
    }

    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), String> {
        let Some(source) = sync_source(store) else { return Ok(()) };
        let device_id = device_id(store)?;
        kosync::push(
            &source,
            &kosync::Update { document: book.document, progress: cfi, percentage: fraction, device: DEVICE_NAME, device_id: &device_id },
        )
    }
}

/// Progress sync through the reader's Dropbox (see dropbox.rs).
struct Dropbox;

impl Transport for Dropbox {
    fn id(&self) -> &'static str {
        "dropbox"
    }

    fn enabled(&self, state: &store::State) -> bool {
        !modules::is_off(state, "capability:progress-sync") && !modules::is_off(state, "capability:dropbox")
    }

    fn pull(&self, _store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, String> {
        Ok(crate::dropbox::pull(book.fingerprint))
    }

    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), String> {
        let local = store.read(|s| s.positions.clone());
        let device_id = device_id(store)?;
        crate::dropbox::push(&local, book.fingerprint, cfi, fraction, DEVICE_NAME, &device_id)
    }

    fn remote(&self, _store: &Store) -> Option<HashMap<String, RemoteRecord>> {
        crate::dropbox::remote_positions()
    }
}

/// Progress sync through the reader's Google Drive (see google.rs).
struct Google;

impl Transport for Google {
    fn id(&self) -> &'static str {
        "google"
    }

    fn enabled(&self, state: &store::State) -> bool {
        !modules::is_off(state, "capability:progress-sync") && !modules::is_off(state, "capability:googledrive")
    }

    fn pull(&self, _store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, String> {
        Ok(crate::google::pull(book.fingerprint))
    }

    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), String> {
        let local = store.read(|s| s.positions.clone());
        let device_id = device_id(store)?;
        crate::google::push(&local, book.fingerprint, cfi, fraction, DEVICE_NAME, &device_id)
    }

    fn remote(&self, _store: &Store) -> Option<HashMap<String, RemoteRecord>> {
        crate::google::remote_positions()
    }
}

static KOSYNC: Kosync = Kosync;
static DROPBOX: Dropbox = Dropbox;
static GOOGLE: Google = Google;

/// Every transport, in merge order (first wins timestamp ties).
fn transports() -> [&'static dyn Transport; 3] {
    [&KOSYNC, &DROPBOX, &GOOGLE]
}

/// The newest remote position across the transports. A transport that fails
/// reports nothing instead of failing the pull, so a dead transport never
/// blocks a live one.
fn pull_merged(store: &Store, transports: &[&dyn Transport], book: &BookKey) -> Option<kosync::Progress> {
    let mut remote = None;
    for t in transports {
        if !store.read(|s| t.enabled(s)) {
            continue;
        }
        if let Ok(found) = t.pull(store, book) {
            remote = newest(remote, found);
        }
    }
    remote
}

/// The newer of two remote positions (a server stamps the time, so the
/// clocks agree); ties keep the earlier transport.
fn newest(current: Option<kosync::Progress>, next: Option<kosync::Progress>) -> Option<kosync::Progress> {
    match (current, next) {
        (None, next) => next,
        (current, None) => current,
        (Some(a), Some(b)) if b.timestamp > a.timestamp => Some(b),
        (Some(a), _) => Some(a),
    }
}

/// Refreshes local positions from one cloud file: the cloud wins per book
/// when it is newer and from another device (this install's own echoes are
/// skipped); books the cloud never saw keep their place. Returns how many
/// books changed.
fn merge_remote(
    local: &mut HashMap<String, Position>,
    remote: &HashMap<String, RemoteRecord>,
    own_device: &str,
) -> usize {
    let mut changed = 0;
    for (fingerprint, record) in remote {
        let Some(here) = local.get_mut(fingerprint) else { continue };
        if record.device_id != own_device && record.time > here.time {
            here.cfi = record.cfi.clone();
            here.fraction = record.fraction;
            here.time = record.time;
            changed += 1;
        }
    }
    changed
}

/// Downloads every cloud's positions file once and refreshes the local
/// positions where the cloud is newer (newest-wins, the same rule as pull on
/// open): Continue reading shows the fresh percent, and opening such a book
/// goes straight to it. Silent: no offers, no pushes, a dead transport is
/// skipped like in a pull.
#[tauri::command(async)]
pub(crate) fn sync_pull_all(store: State<Store>) -> CmdResult<usize> {
    let device = device_id(&store)?;
    let mut updated = 0;
    for t in transports() {
        if !store.read(|s| t.enabled(s)) {
            continue;
        }
        if let Some(remote) = t.remote(&store) {
            store
                .update(|s| {
                    updated += merge_remote(&mut s.positions, &remote, &device);
                })
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(updated)
}

/// What one push round achieved, per transport: the frontend retries only
/// the failed ones, each on its own rhythm.
#[derive(Serialize)]
pub(crate) struct PushReport {
    transport: String,
    error: Option<String>,
}

/// Sends the position through the enabled transports (`only` narrows a
/// retry); every failure is reported instead of failing the push.
fn push_all(
    store: &Store,
    transports: &[&dyn Transport],
    only: &Option<BTreeSet<String>>,
    book: &BookKey,
    cfi: &str,
    fraction: f64,
) -> Vec<PushReport> {
    transports
        .iter()
        .filter(|t| store.read(|s| t.enabled(s)))
        .filter(|t| only.as_ref().is_none_or(|ids| ids.contains(t.id())))
        .map(|t| PushReport { transport: t.id().into(), error: t.push(store, book, cfi, fraction).err() })
        .collect()
}

/// This install's id for the sync server, made once.
fn device_id(store: &Store) -> CmdResult<String> {
    let id = store.read(|s| s.device_id.clone());
    if !id.is_empty() {
        return Ok(id);
    }
    let made = format!("{:016x}", now_ms() ^ u64::from(std::process::id()).rotate_left(32));
    // Checked again under the store's lock: a pull and a push may both get here.
    let mut id = String::new();
    store
        .update(|s| {
            if s.device_id.is_empty() {
                s.device_id = made;
            }
            id = s.device_id.clone();
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}

const DEVICE_NAME: &str = if cfg!(mobile) {
    "Book Reader on Android"
} else if cfg!(windows) {
    "Book Reader on Windows"
} else {
    "Book Reader"
};

/// The window's book's id on the sync server, when its progress syncs
/// (some transport is enabled, and the window keeps positions). The id is
/// the KOSync one; transports that key by fingerprint ignore it.
#[tauri::command(async)]
pub(crate) fn sync_document(window: Window, store: State<Store>) -> CmdResult<Option<String>> {
    let launch = window.state::<ReaderWindows>().get(window.label());
    let (Some(path), false) = (launch.file, launch.no_save) else { return Ok(None) };
    if !store.read(|s| transports().iter().any(|t| t.enabled(s))) {
        return Ok(None);
    }
    kosync::document_id(&path).map(Some).map_err(|e| e.to_string())
}

/// A newer position from another device, offered when a book opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncOffer {
    cfi: String,
    fraction: f64,
    device: String,
    /// When the other device was there (ms since the Unix epoch).
    time: u64,
}

/// What the sync server's position means for the book just opened.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncState {
    /// A newer position from another device, to offer.
    offer: Option<SyncOffer>,
    /// The position here is newer than the server's (a push that didn't get
    /// through, or reading while offline): send it.
    push: bool,
}

/// Asks the sync server about a book (`document` from `sync_document`; its
/// local position under `fingerprint`). A server position is offered only if
/// it comes from another device, differs from the one here and is newer.
#[tauri::command(async)]
pub(crate) fn sync_pull(store: State<Store>, document: String, fingerprint: String) -> CmdResult<SyncState> {
    let book = BookKey { document: &document, fingerprint: &fingerprint };
    let remote = pull_merged(&store, &transports(), &book);
    let local = store.read(|s| s.positions.get(&fingerprint).cloned());
    Ok(compare_positions(local.as_ref(), remote, &device_id(&store)?))
}

/// The newer position wins: the server's is offered when it comes from
/// another device; the local one is pushed when the server lags behind it.
fn compare_positions(local: Option<&Position>, remote: Option<kosync::Progress>, own_device: &str) -> SyncState {
    let Some(remote) = remote else { return SyncState { offer: None, push: local.is_some() } };
    let time = remote.timestamp * 1000;
    let here = local.map_or(0, |here| here.time);
    if local.is_some_and(|here| here.cfi == remote.progress) {
        return SyncState::default();
    }
    if here >= time || remote.device_id == own_device {
        return SyncState { offer: None, push: local.is_some() && here > time };
    }
    SyncState { offer: Some(SyncOffer { cfi: remote.progress, fraction: remote.percentage, device: remote.device, time }), push: false }
}

/// Sends a book's position through the enabled transports (`only` narrows a
/// retry to the transports that still owe an ack).
#[tauri::command(async)]
pub(crate) fn sync_push(
    store: State<Store>,
    document: String,
    fingerprint: String,
    cfi: String,
    fraction: f64,
    only: Option<Vec<String>>,
) -> CmdResult<Vec<PushReport>> {
    let only = only.map(|ids| ids.into_iter().collect::<BTreeSet<_>>());
    let book = BookKey { document: &document, fingerprint: &fingerprint };
    Ok(push_all(&store, &transports(), &only, &book, &cfi, fraction))
}

#[cfg(test)]
mod tests {
    use super::{compare_positions, merge_remote, pull_merged, push_all, sync_source, BookKey, Dropbox, Google, Kosync, Transport};
    use crate::kosync::Progress;
    use crate::store::{self, Position, Store};
    use std::collections::HashMap;

    /// A scripted transport: answers as told and records its calls.
    struct Fake {
        id: &'static str,
        on: bool,
        pull: Result<Option<Progress>, String>,
        push: Result<(), String>,
        calls: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn of(id: &'static str, pull: Result<Option<Progress>, String>, push: Result<(), String>) -> Self {
            Fake { id, on: true, pull, push, calls: std::cell::RefCell::new(Vec::new()) }
        }

        fn off(mut self) -> Self {
            self.on = false;
            self
        }
    }

    impl Transport for Fake {
        fn id(&self) -> &'static str {
            self.id
        }

        fn enabled(&self, _state: &store::State) -> bool {
            self.on
        }

        fn pull(&self, _store: &Store, _book: &BookKey) -> Result<Option<Progress>, String> {
            self.calls.borrow_mut().push("pull");
            self.pull.clone()
        }

        fn push(&self, _store: &Store, _book: &BookKey, _cfi: &str, _fraction: f64) -> Result<(), String> {
            self.calls.borrow_mut().push("push");
            self.push.clone()
        }
    }


    fn here(cfi: &str, time: u64) -> Position {
        Position { cfi: cfi.into(), fraction: 0.5, time }
    }

    fn server(cfi: &str, device_id: &str, timestamp: u64) -> Progress {
        Progress { progress: cfi.into(), percentage: 0.6, device: "phone".into(), device_id: device_id.into(), timestamp }
    }

    fn synced_store(disabled: &[&str]) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(dir.path().join("breader.json"));
        store
            .update(|s| {
                s.sources.push(crate::catalog::Source {
                    id: "1".into(),
                    name: "CWA".into(),
                    url: "https://books.example/opds".into(),
                    user: String::new(),
                    sync: true,
                });
                s.settings.insert(
                    "disabledModules".into(),
                    serde_json::json!(disabled.iter().collect::<Vec<_>>()),
                );
            })
            .unwrap();
        (dir, store)
    }

    #[test]
    fn no_sync_source_while_progress_sync_is_off() {
        let (_dir, store) = synced_store(&[]);
        assert!(sync_source(&store).is_some());
        let (_dir, store) = synced_store(&["capability:progress-sync"]);
        assert!(sync_source(&store).is_none(), "the sync commands no-op");
    }

    #[test]
    fn kosync_needs_a_sync_marked_source() {
        let (_dir, store) = synced_store(&[]);
        assert!(store.read(|s| Kosync.enabled(s)));
        store.update(|s| s.sources.iter_mut().for_each(|o| o.sync = false)).unwrap();
        assert!(!store.read(|s| Kosync.enabled(s)));
    }

    #[test]
    fn dropbox_needs_its_module_and_the_intent() {
        let (_dir, store) = synced_store(&[]);
        assert!(store.read(|s| Dropbox.enabled(s)));
        let (_dir, store) = synced_store(&["capability:dropbox"]);
        assert!(!store.read(|s| Dropbox.enabled(s)));
        let (_dir, store) = synced_store(&["capability:progress-sync"]);
        assert!(!store.read(|s| Dropbox.enabled(s)), "the intent switch gates every transport");
        // KOSync additionally needs catalogs (its own transport rule).
        let (_dir, store) = synced_store(&["capability:catalogs"]);
        assert!(!store.read(|s| Kosync.enabled(s)));
    }

    #[test]
    fn clouds_work_with_catalogs_off() {
        // The spec promises it twice; KOSync alone goes quiet.
        let (_dir, store) = synced_store(&["capability:catalogs"]);
        // Turning catalogs off takes its sources with it (the write guard
        // refuses it while any exist); the intent stays on.
        store.update(|s| s.sources.clear()).unwrap();
        assert!(store.read(|s| Dropbox.enabled(s)));
        assert!(store.read(|s| Google.enabled(s)));
        assert!(!store.read(|s| Kosync.enabled(s)));
    }

    fn cloud_record(cfi: &str, time: u64) -> crate::cloud::RemoteRecord {
        crate::cloud::RemoteRecord {
            cfi: cfi.into(),
            fraction: 0.5,
            time,
            device: "phone".into(),
            device_id: "phone".into(),
        }
    }

    #[test]
    fn startup_pull_takes_newer_remote_positions() {
        let mut local = HashMap::new();
        local.insert("newer-remote".into(), here("a", 100));
        local.insert("newer-local".into(), here("a", 300));
        local.insert("same".into(), here("a", 200));
        local.insert("unseen".into(), here("a", 50));
        let mut remote = HashMap::new();
        remote.insert("newer-remote".into(), cloud_record("b", 200));
        remote.insert("newer-local".into(), cloud_record("b", 100));
        remote.insert("same".into(), cloud_record("a", 200));
        remote.insert("elsewhere".into(), cloud_record("c", 400));
        assert_eq!(merge_remote(&mut local, &remote, "pc"), 1);
        assert_eq!(local["newer-remote"].cfi, "b");
        assert_eq!(local["newer-remote"].time, 200);
        assert_eq!(local["newer-local"].cfi, "a");
        assert_eq!(local["same"].time, 200);
        assert!(
            !local.contains_key("elsewhere"),
            "books never opened here gain nothing"
        );
    }

    #[test]
    fn startup_pull_ignores_this_installs_echoes() {
        let mut local = HashMap::new();
        local.insert("fp".into(), here("a", 100));
        let mut remote = HashMap::new();
        let mut echo = cloud_record("b", 500);
        echo.device_id = "pc".into();
        remote.insert("fp".into(), echo);
        assert_eq!(merge_remote(&mut local, &remote, "pc"), 0);
        assert_eq!(local["fp"].cfi, "a");
    }

    #[test]
    fn google_needs_its_module_and_the_intent() {
        let (_dir, store) = synced_store(&[]);
        assert!(store.read(|s| Google.enabled(s)));
        let (_dir, store) = synced_store(&["capability:googledrive"]);
        assert!(!store.read(|s| Google.enabled(s)));
        let (_dir, store) = synced_store(&["capability:progress-sync"]);
        assert!(!store.read(|s| Google.enabled(s)), "the intent switch gates every transport");
    }

    #[test]
    fn a_newer_position_from_another_device_is_offered() {
        let state = compare_positions(Some(&here("a", 1_000)), Some(server("b", "phone", 2)), "pc");
        assert_eq!(state.offer.map(|o| o.cfi).as_deref(), Some("b"));
        assert!(!state.push);
        let state = compare_positions(None, Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_some(), "a book not read here yet");
    }

    #[test]
    fn a_newer_local_position_is_pushed() {
        let state = compare_positions(Some(&here("a", 3_000)), Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_none() && state.push);
        let state = compare_positions(Some(&here("a", 1)), None, "pc");
        assert!(state.offer.is_none() && state.push, "the server has nothing yet");
    }

    #[test]
    fn the_same_place_or_this_devices_own_position_changes_nothing() {
        let state = compare_positions(Some(&here("b", 1_000)), Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_none() && !state.push);
        let state = compare_positions(Some(&here("a", 1_000)), Some(server("b", "pc", 2)), "pc");
        assert!(state.offer.is_none() && !state.push);
    }

    const DOC: &str = "doc";
    const FP: &str = "fp";

    fn book() -> BookKey<'static> {
        BookKey { document: DOC, fingerprint: FP }
    }

    fn as_transports<'a>(fakes: &[&'a Fake]) -> Vec<&'a dyn Transport> {
        fakes
            .iter()
            .map(|f| {
                let t: &'a dyn Transport = *f;
                t
            })
            .collect()
    }

    #[test]
    fn fan_out_ignores_a_down_transport() {
        let (_dir, store) = synced_store(&[]);
        let live = Fake::of("live", Ok(Some(server("b", "phone", 2))), Ok(()));
        let dead = Fake::of("dead", Err("gone".into()), Err("gone".into()));
        let remote = pull_merged(&store, &as_transports(&[&live, &dead]), &book());
        assert_eq!(remote.map(|p| p.progress).as_deref(), Some("b"));
        assert_eq!(live.calls.borrow().as_slice(), ["pull"]);
        assert_eq!(dead.calls.borrow().as_slice(), ["pull"], "asked, then isolated");
    }

    #[test]
    fn newest_wins_across_transports_and_ties_keep_order() {
        let (_dir, store) = synced_store(&[]);
        let older = Fake::of("older", Ok(Some(server("a", "phone", 3))), Ok(()));
        let newer = Fake::of("newer", Ok(Some(server("b", "phone", 5))), Ok(()));
        let remote = pull_merged(&store, &as_transports(&[&older, &newer]), &book());
        assert_eq!(remote.map(|p| p.progress).as_deref(), Some("b"));
        let remote = pull_merged(&store, &as_transports(&[&newer, &older]), &book());
        assert_eq!(remote.map(|p| p.progress).as_deref(), Some("b"), "time decides, not order");
        let tie = Fake::of("tie", Ok(Some(server("c", "phone", 5))), Ok(()));
        let remote = pull_merged(&store, &as_transports(&[&newer, &tie]), &book());
        assert_eq!(remote.map(|p| p.progress).as_deref(), Some("b"), "ties keep the first");
    }

    #[test]
    fn disabled_transports_are_skipped() {
        let (_dir, store) = synced_store(&[]);
        let off = Fake::of("off", Ok(Some(server("b", "phone", 2))), Ok(())).off();
        let remote = pull_merged(&store, &as_transports(&[&off]), &book());
        assert!(remote.is_none());
        assert!(off.calls.borrow().is_empty());
        let reports = push_all(&store, &as_transports(&[&off]), &None, &book(), "a", 0.5);
        assert!(reports.is_empty());
        assert!(off.calls.borrow().is_empty());
    }

    #[test]
    fn push_reports_failures_and_honors_only() {
        let (_dir, store) = synced_store(&[]);
        let ok = Fake::of("ok", Ok(None), Ok(()));
        let failing = Fake::of("failing", Ok(None), Err("down".into()));
        let reports = push_all(&store, &as_transports(&[&ok, &failing]), &None, &book(), "a", 0.5);
        assert_eq!(reports.len(), 2);
        assert_eq!((reports[0].transport.as_str(), reports[0].error.as_deref()), ("ok", None));
        assert_eq!((reports[1].transport.as_str(), reports[1].error.as_deref()), ("failing", Some("down")));
        let only: Option<std::collections::BTreeSet<String>> = Some(["failing".to_string()].into_iter().collect());
        let reports = push_all(&store, &as_transports(&[&ok, &failing]), &only, &book(), "a", 0.5);
        assert_eq!(reports.len(), 1);
        assert_eq!(ok.calls.borrow().as_slice(), ["push"], "clean transports are not retried");
        assert_eq!(failing.calls.borrow().as_slice(), ["push", "push"]);
    }
}
