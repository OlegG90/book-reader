//! Reading progress sync (KOSync): what the server says about a book.

use crate::catalog;
use crate::clock::now_ms;
use crate::cloud::{self, CloudStore, RemoteRecord};
use crate::fingerprint;
use crate::formats;
use crate::kosync;
use crate::library_cmds::{library_root, LibraryBooks};
use crate::modules;
use crate::naming;
use crate::platform::{Active, Platform};
use crate::store::{self, Job, Position, Store};
use crate::sync_error::SyncError;
use crate::windows::ReaderWindows;
use crate::CmdResult;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use tauri::{Manager, State, Window};

mod engine;
pub(crate) use engine::{start as start_engine, SyncEngine};

/// The source that syncs progress, if one is set and sync is on: with the
/// module off the sync commands no-op (no network) instead of failing.
fn sync_source(store: &Store) -> Option<catalog::Source> {
    if store.read(|s| modules::is_off(s, modules::id::PROGRESS_SYNC)) {
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
    fn pull(&self, store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, SyncError>;
    /// Sends the position; a cloud without a login answers `NotConnected`.
    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), SyncError>;
    /// The cloud's whole positions file, for the startup pull; transports
    /// without one (KOSync) have nothing to fetch in bulk.
    fn remote(&self, _store: &Store) -> Option<HashMap<String, RemoteRecord>> {
        None
    }
    /// Whether the transport can reach its store: a cloud needs a login (a
    /// refresh token is kept); KOSync needs nothing more than being enabled.
    /// Asked before book work, so no file is read and hashed for nothing;
    /// positions learn it from the `NotConnected` answer instead.
    fn connected(&self) -> bool {
        true
    }
    /// Whether the transport keeps book files (the clouds do, KOSync doesn't).
    fn keeps_books(&self) -> bool {
        false
    }
    /// The cloud's book manifest, if the transport keeps books.
    fn books(&self) -> Option<HashMap<String, crate::cloud::BookRecord>> {
        None
    }
    /// How much of the cloud is used and how much there is (bytes), if it says.
    fn space(&self) -> Option<(u64, u64)> {
        None
    }
    /// Removes a book from the cloud for every device (a removal record in
    /// this device's manifest, then the file goes).
    fn remove_book(&self, store: &Store, fingerprint: &str, record: &crate::cloud::BookRecord) -> Result<(), SyncError> {
        let _ = (store, fingerprint, record);
        Ok(())
    }
    /// Downloads a book file by its cloud name; `None` when the cloud never
    /// saw it.
    fn pull_book(&self, remote_name: &str) -> Result<Option<Vec<u8>>, SyncError> {
        let _ = remote_name;
        Ok(None)
    }
    /// Uploads a book file the cloud lacks, and records it in this device's
    /// manifest there; true when the cloud gained the book.
    fn push_book(&self, store: &Store, fingerprint: &str, record: &crate::cloud::BookRecord, bytes: &[u8]) -> Result<bool, SyncError> {
        let _ = (store, fingerprint, record, bytes);
        Ok(false)
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
        !modules::is_off(state, modules::id::PROGRESS_SYNC)
            && !modules::is_off(state, modules::id::CATALOGS)
            && state.sources.iter().any(|s| s.sync)
    }

    fn pull(&self, store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, SyncError> {
        let Some(source) = sync_source(store) else { return Ok(None) };
        Ok(kosync::pull(&source, book.document)?)
    }

    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), SyncError> {
        let Some(source) = sync_source(store) else { return Ok(()) };
        let device_id = device_id(store)?;
        Ok(kosync::push(
            &source,
            &kosync::Update { document: book.document, progress: cfi, percentage: fraction, device: DEVICE_NAME, device_id: &device_id },
        )?)
    }
}

/// Progress and book files through a cloud store (Dropbox, Google Drive):
/// the same for every cloud, see cloud.rs.
struct CloudTransport<'a>(&'a dyn CloudStore);

impl Transport for CloudTransport<'_> {
    fn id(&self) -> &'static str {
        self.0.id()
    }

    fn enabled(&self, state: &store::State) -> bool {
        !modules::is_off(state, modules::id::PROGRESS_SYNC) && !modules::is_off(state, self.0.module())
    }

    fn pull(&self, _store: &Store, book: &BookKey) -> Result<Option<kosync::Progress>, SyncError> {
        Ok(cloud::pull(self.0, book.fingerprint))
    }

    fn push(&self, store: &Store, book: &BookKey, cfi: &str, fraction: f64) -> Result<(), SyncError> {
        let local = store.read(|s| s.positions.clone());
        let device_id = device_id(store)?;
        cloud::push(self.0, &local, book.fingerprint, cfi, fraction, DEVICE_NAME, &device_id)
    }

    fn remote(&self, _store: &Store) -> Option<HashMap<String, RemoteRecord>> {
        cloud::remote_positions(self.0)
    }

    fn connected(&self) -> bool {
        self.0.connected()
    }

    fn keeps_books(&self) -> bool {
        true
    }

    fn books(&self) -> Option<HashMap<String, cloud::BookRecord>> {
        cloud::book_manifest(self.0)
    }

    fn pull_book(&self, remote_name: &str) -> Result<Option<Vec<u8>>, SyncError> {
        cloud::download_book(self.0, remote_name)
    }

    fn push_book(&self, store: &Store, fingerprint: &str, record: &cloud::BookRecord, bytes: &[u8]) -> Result<bool, SyncError> {
        cloud::push_book(self.0, &device_id(store)?, fingerprint, record, bytes)
    }

    fn remove_book(&self, store: &Store, fingerprint: &str, record: &cloud::BookRecord) -> Result<(), SyncError> {
        cloud::remove_book(self.0, &device_id(store)?, fingerprint, record)
    }

    fn space(&self) -> Option<(u64, u64)> {
        cloud::space(self.0)
    }
}

static KOSYNC: Kosync = Kosync;
static DROPBOX: CloudTransport<'static> = CloudTransport(&crate::dropbox::STORE);
static GOOGLE: CloudTransport<'static> = CloudTransport(&crate::google::STORE);

/// Every transport, in merge order (first wins timestamp ties).
fn transports() -> [&'static dyn Transport; 3] {
    [&KOSYNC, &DROPBOX, &GOOGLE]
}

/// Whether a transport's modules are on.
fn enabled(store: &Store, t: &dyn Transport) -> bool {
    store.read(|s| t.enabled(s))
}

/// The transports among `transports` that keep book files and can take
/// part now: enabled and connected (asked up front, so no book file is read
/// and hashed for nothing).
fn book_stores<'a>(store: &Store, transports: &[&'a dyn Transport]) -> Vec<&'a dyn Transport> {
    transports.iter().copied().filter(|t| t.keeps_books() && enabled(store, *t) && t.connected()).collect()
}

/// The newest remote position across the transports. A transport that fails
/// reports nothing instead of failing the pull, so a dead transport never
/// blocks a live one.
fn pull_merged(store: &Store, transports: &[&dyn Transport], book: &BookKey) -> Option<kosync::Progress> {
    let mut remote = None;
    for t in transports {
        if !enabled(store, *t) {
            continue;
        }
        if let Ok(found) = t.pull(store, book) {
            remote = newest(remote, found);
        }
    }
    remote
}

/// The newer of two remote positions; ties keep the earlier transport. KOSync
/// times come from the server, cloud records from each device's clock.
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
/// skipped); books the cloud never saw keep their place. A library book never
/// opened here (say, one just downloaded) takes the cloud's place too, so it
/// opens there without a jump offer. Returns how many books changed.
fn merge_remote(
    local: &mut HashMap<String, Position>,
    remote: &HashMap<String, RemoteRecord>,
    own_device: &str,
    library: &BTreeSet<String>,
) -> usize {
    let mut changed = 0;
    for (fingerprint, record) in remote {
        // A book with no position here takes one only when it is in the library.
        let newer = match local.get(fingerprint) {
            Some(here) => record.time > here.time,
            None => library.contains(fingerprint) && record.time > 0,
        };
        if newer && record.device_id != own_device {
            let by = Some(crate::store::Writer { device: record.device.clone(), device_id: record.device_id.clone() });
            local.insert(fingerprint.clone(), Position { cfi: record.cfi.clone(), fraction: record.fraction, time: record.time, by });
            changed += 1;
        }
    }
    changed
}

/// Merges every enabled cloud's positions file into the local positions, for
/// the whole library (newest-wins, the same rule as pull on open): Continue
/// reading shows the fresh percent, and opening such a book goes straight to
/// it. The engine runs it once per start; returns how many books changed.
fn pull_all_clouds(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport]) -> CmdResult<usize> {
    let library: BTreeSet<String> =
        library_root(store).map(|root| books.snapshot(store, &root).into_iter().map(|b| b.fingerprint).collect()).unwrap_or_default();
    let enabled_now: Vec<_> = transports.iter().copied().filter(|t| enabled(store, *t)).collect();
    merge_cloud_positions(store, &enabled_now, &library)
}

/// Merges these clouds' positions files into the local positions; books in
/// `library` never opened here take the cloud's place too. Returns how many
/// books changed.
fn merge_cloud_positions(store: &Store, clouds: &[&dyn Transport], library: &BTreeSet<String>) -> CmdResult<usize> {
    let device = device_id(store)?;
    let mut updated = 0;
    for t in clouds {
        if let Some(remote) = t.remote(store) {
            store
                .update(|s| {
                    updated += merge_remote(&mut s.positions, &remote, &device, library);
                })
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(updated)
}

/// Uploads a library book's file to every cloud that keeps books and lacks it;
/// true when one gained it. A cloud's failure is reported only when no cloud
/// gained the book.
fn push_book_to_cloud(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport], fingerprint: &str) -> Result<bool, SyncError> {
    let clouds = book_stores(store, transports);
    if clouds.is_empty() {
        return Ok(false);
    }
    let root = library_root(store)?;
    let entry = books.snapshot(store, &root).into_iter().find(|b| b.fingerprint == fingerprint);
    let Some(entry) = entry else { return Ok(false) };
    let path = entry.absolute(&root);
    // The scan may be older than the file: never upload changed bytes.
    if fingerprint::fingerprint(&path).map_err(|e| e.to_string())? != fingerprint {
        return Ok(false);
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let record = crate::cloud::BookRecord {
        name: entry.path.clone(),
        size: bytes.len() as u64,
        sha256: fingerprint::full_sha256(&path).map_err(|e| e.to_string())?,
        added: now_ms(),
        removed: None,
    };
    let mut pushed = false;
    let mut failed = None;
    // One cloud down never keeps the book from the others.
    for t in clouds {
        match t.push_book(store, fingerprint, &record, &bytes) {
            Ok(gained) => pushed |= gained,
            // A full cloud outweighs another failure: the book waits for space.
            Err(e) if failed.as_ref().is_none_or(|f: &SyncError| f.kind != crate::sync_error::Kind::NoSpace) => failed = Some(e),
            Err(_) => {}
        }
    }
    match failed {
        Some(e) if !pushed => Err(e),
        _ => Ok(pushed),
    }
}

/// Book files travel only on unmetered networks when the setting says so
/// (on by default); positions are tiny and always sync. The desktop counts
/// every network as unmetered.
fn book_transfer_allowed(store: &Store) -> bool {
    book_transfer_gate(
        store.read(|s| s.settings.get("booksWifiOnly").and_then(|v| v.as_bool()).unwrap_or(true)),
        Active::network_metered(),
    )
}

/// The Wi-Fi gate as a pure rule, for tests: holding books back needs both
/// the setting on and a metered network.
fn book_transfer_gate(wifi_only: bool, metered: bool) -> bool {
    !wifi_only || !metered
}

/// A cloud book the library lacks, for the download offer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MissingBook {
    fingerprint: String,
    name: String,
    size: u64,
}

/// What the clouds say about each book, the latest event across them
/// winning (an upload, or a removal from the clouds); `None` when no cloud
/// that keeps books could be read.
fn latest_books(store: &Store, transports: &[&dyn Transport]) -> Option<HashMap<String, crate::cloud::BookRecord>> {
    latest_of(&cloud_books(store, transports))
}

/// Each cloud's books (removed ones included), for the clouds that keep
/// books and could be read.
fn cloud_books(store: &Store, transports: &[&dyn Transport]) -> Vec<HashMap<String, crate::cloud::BookRecord>> {
    book_stores(store, transports).iter().filter_map(|t| t.books()).collect()
}

/// The latest event per book across clouds; `None` without any cloud.
fn latest_of(per_cloud: &[HashMap<String, crate::cloud::BookRecord>]) -> Option<HashMap<String, crate::cloud::BookRecord>> {
    if per_cloud.is_empty() {
        return None;
    }
    let mut all: HashMap<String, crate::cloud::BookRecord> = HashMap::new();
    for books in per_cloud {
        for (fingerprint, record) in books {
            if all.get(fingerprint).is_none_or(|have| record.event_time() > have.event_time()) {
                all.insert(fingerprint.clone(), record.clone());
            }
        }
    }
    Some(all)
}

/// The cloud books the library lacks: present in the clouds, not removed
/// here on purpose, and a book file this device opens.
fn collect_missing(
    store: &Store,
    latest: &HashMap<String, crate::cloud::BookRecord>,
    local: &BTreeSet<String>,
    removed_here: &BTreeSet<String>,
) -> Vec<MissingBook> {
    latest
        .iter()
        .filter(|(fingerprint, record)| {
            record.removed.is_none() && !local.contains(*fingerprint) && !removed_here.contains(*fingerprint) && book_format_allowed(store, &record.name).is_ok()
        })
        .map(|(fingerprint, record)| MissingBook { fingerprint: fingerprint.clone(), name: record.name.clone(), size: record.size })
        .collect()
}

/// The cloud books the library lacks, for the download offer. Silent like a
/// pull: transports that are off or disconnected report nothing.
#[tauri::command(async)]
pub(crate) fn sync_missing_books(store: State<Store>, books: State<LibraryBooks>) -> CmdResult<Vec<MissingBook>> {
    missing_cloud_books(&store, &books, &transports())
}

/// The books the clouds hold and the library lacks, leaving out names this
/// device wouldn't open.
fn missing_cloud_books(store: &Store, books: &LibraryBooks, transports: &[&dyn Transport]) -> CmdResult<Vec<MissingBook>> {
    let root = library_root(store)?;
    let local: BTreeSet<String> = books.snapshot(store, &root).into_iter().map(|b| b.fingerprint).collect();
    let Some(latest) = latest_books(store, transports) else { return Ok(Vec::new()) };
    let removed_here = store.read(|s| s.removed_here.clone());
    Ok(collect_missing(store, &latest, &local, &removed_here))
}

/// A manifest name is a library-relative path: absolute paths and parent
/// climbs never land in the library, whatever the cloud claims.
fn safe_relative(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    if path.is_absolute() || path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("The cloud name is not a library path.".into());
    }
    Ok(path.to_path_buf())
}

/// Where a downloaded book lands: its manifest name, unless a different
/// book already holds it — then ` (2)`, like Organize (`naming::place`).
/// `None` when the library already has this very book there. Names that
/// aren't book files the app opens never land, whatever the cloud claims.
fn place_book(root: &Path, name: &str, fingerprint: &str) -> CmdResult<Option<PathBuf>> {
    safe_relative(name)?;
    formats::split(name).ok_or("The cloud name is not a book file.")?;
    Ok(match naming::place(root, name, fingerprint, &HashSet::new()).map_err(|e| e.to_string())? {
        naming::Placement::Free(rel) => Some(root.join(rel)),
        naming::Placement::Exists(_) => None,
    })
}

/// Places a verified download in the library: picks its name and writes it,
/// trying the next free name when another file appears there meanwhile.
/// False when the library already holds the book.
fn land_book(root: &Path, name: &str, fingerprint: &str, bytes: &[u8]) -> CmdResult<bool> {
    for _ in 0..3 {
        let Some(dest) = place_book(root, name, fingerprint)? else { return Ok(false) };
        match naming::write_new(&dest, bytes) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("No free name for the downloaded book.".into())
}

/// Whether a cloud name is a book file in a format turned on in Modules; the
/// refusal says why, as a catalog download does.
fn book_format_allowed(store: &Store, name: &str) -> CmdResult<()> {
    let (_, reader) = formats::split(name).ok_or("The cloud name is not a book file.")?;
    store.read(|s| formats::check(s, reader))
}

/// Downloads one book from the first cloud with a good copy and lands it in the
/// library; true when it landed.
fn pull_book_from_cloud(store: &Store, transports: &[&dyn Transport], fingerprint: &str) -> CmdResult<bool> {
    let root = library_root(store)?;
    let mut failed = None;
    // A cloud that fails, or sends a copy that fails its hash, hands over to the next.
    for t in book_stores(store, transports) {
        let Some(manifest) = t.books() else { continue };
        let Some(record) = manifest.get(fingerprint).filter(|r| r.removed.is_none()) else { continue };
        if let Err(e) = book_format_allowed(store, &record.name) {
            failed = Some(e);
            continue;
        }
        let remote_name = crate::cloud::remote_book_name(fingerprint, &record.name);
        let bytes = match t.pull_book(&remote_name) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(e) => {
                failed = Some(e.message);
                continue;
            }
        };
        // Same bytes as recorded, or nothing lands in the library.
        if fingerprint::sha256_bytes(&bytes) != record.sha256 {
            failed = Some("The cloud copy failed its hash check.".into());
            continue;
        }
        let landed = land_book(&root, &record.name, fingerprint, &bytes)?;
        if landed {
            // The new book opens where the other device is, not with a jump
            // offer: its position comes from the cloud that sent it.
            let _ = merge_cloud_positions(store, &[t], &[fingerprint.to_string()].into_iter().collect());
        }
        return Ok(landed);
    }
    failed.map_or(Ok(false), Err)
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

/// How this device names itself to the other devices (in the jump offer).
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

/// Queues a book's position for every enabled transport; the engine sends
/// it, and retries a transport that fails on its own.
#[tauri::command(async)]
pub(crate) fn sync_queue_position(
    store: State<Store>,
    engine: State<SyncEngine>,
    document: String,
    fingerprint: String,
    cfi: String,
    fraction: f64,
) -> CmdResult<()> {
    let ids: Vec<&str> = transports().into_iter().filter(|t| enabled(&store, *t)).map(|t| t.id()).collect();
    queue_jobs(
        &store,
        &engine,
        ids.into_iter().map(|id| Job::Position {
            transport: id.into(),
            fingerprint: fingerprint.clone(),
            document: document.clone(),
            cfi: cfi.clone(),
            fraction,
        }),
    )
}

/// Removes books from every cloud, for every device: they leave the other
/// devices' libraries too, and Removed books forgets them.
#[tauri::command(async)]
pub(crate) fn sync_remove_from_clouds(store: State<Store>, engine: State<SyncEngine>, fingerprints: Vec<String>) -> CmdResult<()> {
    // The books stay in removed_here until every cloud has the removal (the
    // engine drops them then), so none is downloaded back meanwhile.
    queue_jobs(&store, &engine, fingerprints.into_iter().map(|fingerprint| Job::Remove { fingerprint }))
}

/// The books deleted from this library that the clouds still hold: the
/// Removed books list, from which they can be removed from the clouds too.
#[tauri::command(async)]
pub(crate) fn sync_removed_books(store: State<Store>) -> Vec<MissingBook> {
    // A book on its way out of the clouds is no longer listed.
    let removed_here: BTreeSet<String> = store.read(|s| {
        let leaving: BTreeSet<&str> = s.outbox.iter().filter_map(|p| match &p.job {
            Job::Remove { fingerprint } => Some(fingerprint.as_str()),
            _ => None,
        }).collect();
        s.removed_here.iter().filter(|f| !leaving.contains(f.as_str())).cloned().collect()
    });
    if removed_here.is_empty() {
        return Vec::new();
    }
    let latest = latest_books(&store, &transports()).unwrap_or_default();
    removed_here
        .iter()
        .filter_map(|fingerprint| {
            let record = latest.get(fingerprint).filter(|r| r.removed.is_none())?;
            Some(MissingBook { fingerprint: fingerprint.clone(), name: record.name.clone(), size: record.size })
        })
        .collect()
}

/// Brings books from Removed books back into this library: they are no
/// longer removed here, and their files come down from the clouds (on Wi-Fi,
/// if the reader asked for that). The windows hear `sync-books-arrived` as
/// they land.
#[tauri::command(async)]
pub(crate) fn sync_restore_books(store: State<Store>, engine: State<SyncEngine>, fingerprints: Vec<String>) -> CmdResult<()> {
    restore_books(&store, &fingerprints)?;
    engine.wake();
    Ok(())
}

/// Takes books off Removed books and queues their downloads, in one write so
/// a reconcile in between can't see one without the other.
fn restore_books(store: &Store, fingerprints: &[String]) -> CmdResult<()> {
    store
        .update(|s| {
            for fingerprint in fingerprints {
                s.removed_here.remove(fingerprint);
                engine::queue(&mut s.outbox, Job::Download { fingerprint: fingerprint.clone() });
            }
        })
        .map_err(|e| e.to_string())
}

/// Queues downloads of cloud books the library lacks; the windows hear
/// `sync-books-arrived` as they land. Held on metered networks until Wi-Fi is back.
#[tauri::command(async)]
pub(crate) fn sync_queue_downloads(store: State<Store>, engine: State<SyncEngine>, fingerprints: Vec<String>) -> CmdResult<()> {
    queue_jobs(&store, &engine, fingerprints.into_iter().map(|fingerprint| Job::Download { fingerprint }))
}

/// Each library book's place in the clouds, for its card: `cloud` (every
/// connected cloud holds it), `waiting` (its upload is queued) or `nofit`
/// (a cloud is full). Books with no state yet are left out. No network: it
/// reads what the engine last learned.
#[tauri::command]
pub(crate) fn sync_book_states(store: State<Store>, engine: State<SyncEngine>) -> HashMap<String, &'static str> {
    let mut states: HashMap<String, &'static str> = engine.in_clouds().into_iter().map(|fingerprint| (fingerprint, "cloud")).collect();
    store.read(|s| {
        for pending in &s.outbox {
            if let Job::Upload { fingerprint } = &pending.job {
                states.insert(fingerprint.clone(), if pending.no_room { "nofit" } else { "waiting" });
            }
        }
    });
    states
}

/// Has the engine look at its outbox now (the app came back to the front).
#[tauri::command]
pub(crate) fn sync_wake(engine: State<SyncEngine>) {
    engine.wake();
}

/// Sends the positions still owed, before the last window closes: the engine's
/// worker ends with the app, so that window can't leave them to it. While
/// other windows stay open the engine is still running and sends them itself.
#[tauri::command(async)]
pub(crate) fn sync_flush(window: Window, store: State<Store>, books: State<LibraryBooks>, windows: State<ReaderWindows>) {
    if windows.others_than(window.label()) {
        return;
    }
    engine::run_positions(&store, &books, &transports(), now_ms());
}

/// Whether the engine's startup pull changed positions, for a window that
/// started listening for `sync-positions-updated` after it was said.
#[tauri::command]
pub(crate) fn sync_pulled(engine: State<SyncEngine>) -> bool {
    engine.pulled()
}

/// Puts jobs in the outbox and wakes the engine.
fn queue_jobs(store: &Store, engine: &SyncEngine, jobs: impl IntoIterator<Item = Job>) -> CmdResult<()> {
    store
        .update(|s| {
            for job in jobs {
                engine::queue(&mut s.outbox, job);
            }
        })
        .map_err(|e| e.to_string())?;
    engine.wake();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{book_format_allowed, book_transfer_allowed, book_transfer_gate, collect_missing, compare_positions, land_book, merge_remote, missing_cloud_books, place_book, pull_all_clouds, pull_book_from_cloud, pull_merged, push_book_to_cloud, safe_relative, sync_source, transports, BookKey, CloudTransport, Kosync, Transport, DROPBOX, GOOGLE};
    use crate::cloud::CloudStore;
    use crate::sync_error::{Kind, SyncError};
    use crate::store::Job;
    use crate::library_cmds::library_root;
    use crate::library_cmds::LibraryBooks;
    use crate::cloud::push_book_missing;
    use crate::kosync::Progress;
    use crate::store::{self, Position, Store};
    use std::collections::HashMap;

    /// A scripted transport: answers as told and records its calls.
    struct Fake {
        id: &'static str,
        on: bool,
        linked: bool,
        pull: Result<Option<Progress>, String>,
        push: Result<(), String>,
        calls: std::cell::RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn of(id: &'static str, pull: Result<Option<Progress>, String>, push: Result<(), String>) -> Self {
            Fake { id, on: true, linked: true, pull, push, calls: std::cell::RefCell::new(Vec::new()) }
        }

        fn off(mut self) -> Self {
            self.on = false;
            self
        }

        fn disconnected(mut self) -> Self {
            self.linked = false;
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

        fn connected(&self) -> bool {
            self.linked
        }

        fn pull(&self, _store: &Store, _book: &BookKey) -> Result<Option<Progress>, SyncError> {
            if !self.linked {
                return Err(SyncError::new(Kind::NotConnected, "not connected"));
            }
            self.calls.borrow_mut().push("pull");
            Ok(self.pull.clone()?)
        }

        fn push(&self, _store: &Store, _book: &BookKey, _cfi: &str, _fraction: f64) -> Result<(), SyncError> {
            if !self.linked {
                return Err(SyncError::new(Kind::NotConnected, "not connected"));
            }
            self.calls.borrow_mut().push("push");
            Ok(self.push.clone()?)
        }
    }


    fn here(cfi: &str, time: u64) -> Position {
        Position { cfi: cfi.into(), fraction: 0.5, time, by: None }
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
        let (_dir, store) = synced_store(&[crate::modules::id::PROGRESS_SYNC]);
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
        assert!(store.read(|s| DROPBOX.enabled(s)));
        let (_dir, store) = synced_store(&[crate::modules::id::DROPBOX]);
        assert!(!store.read(|s| DROPBOX.enabled(s)));
        let (_dir, store) = synced_store(&[crate::modules::id::PROGRESS_SYNC]);
        assert!(!store.read(|s| DROPBOX.enabled(s)), "the intent switch gates every transport");
        // KOSync additionally needs catalogs (its own transport rule).
        let (_dir, store) = synced_store(&[crate::modules::id::CATALOGS]);
        assert!(!store.read(|s| Kosync.enabled(s)));
    }

    #[test]
    fn clouds_work_with_catalogs_off() {
        // The spec promises it twice; KOSync alone goes quiet.
        let (_dir, store) = synced_store(&[crate::modules::id::CATALOGS]);
        // Turning catalogs off takes its sources with it (the write guard
        // refuses it while any exist); the intent stays on.
        store.update(|s| s.sources.clear()).unwrap();
        assert!(store.read(|s| DROPBOX.enabled(s)));
        assert!(store.read(|s| GOOGLE.enabled(s)));
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
        remote.insert("downloaded".into(), cloud_record("d", 400));
        let library = ["downloaded".to_string()].into_iter().collect();
        assert_eq!(merge_remote(&mut local, &remote, "pc", &library), 2);
        assert_eq!(local["downloaded"].cfi, "d", "a library book never opened here takes the cloud's place");
        assert_eq!(local["newer-remote"].cfi, "b");
        assert_eq!(local["newer-remote"].time, 200);
        assert_eq!(local["newer-local"].cfi, "a");
        assert_eq!(local["same"].time, 200);
        assert!(
            !local.contains_key("elsewhere"),
            "books outside the library gain nothing"
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
        assert_eq!(merge_remote(&mut local, &remote, "pc", &["fp".to_string()].into_iter().collect()), 0);
        assert_eq!(local["fp"].cfi, "a");
    }

    fn book_record(name: &str, size: u64) -> crate::cloud::BookRecord {
        crate::cloud::BookRecord { name: name.into(), size, sha256: "abc".into(), added: 0, removed: None }
    }

    #[test]
    fn wifi_gate_needs_both() {
        assert!(book_transfer_gate(false, false));
        assert!(book_transfer_gate(false, true));
        assert!(book_transfer_gate(true, false));
        assert!(!book_transfer_gate(true, true));
    }

    #[test]
    fn wifi_setting_defaults_to_waiting() {
        // Absent key means on, like the frontend default.
        let (_dir, store) = synced_store(&[]);
        assert!(store.read(|s| s.settings.get("booksWifiOnly").is_none()));
        assert!(book_transfer_allowed(&store));
        store.update(|s| { s.settings.insert("booksWifiOnly".into(), serde_json::json!(false)); }).unwrap();
        assert!(book_transfer_allowed(&store));
    }

    #[test]
    fn collect_missing_lists_each_book_once() {
        let mut a = HashMap::new();
        a.insert("gone".into(), book_record("A.epub", 1));
        a.insert("here".into(), book_record("H.epub", 1));
        let mut b = HashMap::new();
        b.insert("gone".into(), book_record("A.epub", 1));
        b.insert("other".into(), book_record("O.epub", 2));
        let (_dir, store) = synced_store(&[]);
        let mut latest = a;
        latest.extend(b);
        let mut removed = book_record("R.epub", 3);
        removed.removed = Some(5);
        latest.insert("removed".into(), removed);
        latest.insert("dropped-here".into(), book_record("D.epub", 4));
        let local: std::collections::BTreeSet<String> = ["here".into()].into_iter().collect();
        let removed_here: std::collections::BTreeSet<String> = ["dropped-here".into()].into_iter().collect();
        let missing = collect_missing(&store, &latest, &local, &removed_here);
        assert_eq!(missing.len(), 2, "neither removed from the clouds nor removed here");
        assert!(missing.iter().any(|m| m.fingerprint == "gone" && m.name == "A.epub"));
        assert!(missing.iter().any(|m| m.fingerprint == "other"));
    }

    #[test]
    fn cloud_names_must_stay_inside_the_library() {
        assert!(safe_relative("Author/Book.epub").is_ok());
        assert!(safe_relative("/etc/passwd").is_err());
        assert!(safe_relative("../outside.epub").is_err());
        assert!(safe_relative("a/../../b.epub").is_err());
    }

    #[test]
    fn occupied_names_gain_a_counter() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Free name passes through.
        let dest = place_book(root, "Author/Book.epub", "fp").unwrap().unwrap();
        assert_eq!(dest, root.join("Author/Book.epub"));
        // Same book already there: nothing to do.
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&dest).unwrap();
        file.write_all(b"same").unwrap();
        let fp = crate::fingerprint::fingerprint(&dest).unwrap();
        assert_eq!(place_book(root, "Author/Book.epub", &fp).unwrap(), None);
        // Another book under the same name: " (2)", like Organize.
        let dest2 = place_book(root, "Author/Book.epub", "other").unwrap().unwrap();
        assert_eq!(dest2, root.join("Author/Book (2).epub"));
    }

    #[test]
    fn cloud_books_need_a_book_name_in_a_format_that_is_on() {
        let (_dir, store) = synced_store(&[crate::modules::id::FB2]);
        assert!(book_format_allowed(&store, "A/B.epub").is_ok());
        assert!(book_format_allowed(&store, "A/B.fb2").is_err(), "its module is off");
        assert!(book_format_allowed(&store, "A/run.exe").is_err());
    }

    #[test]
    fn only_book_files_land_and_never_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(place_book(root, "Author/run.exe", "fp").is_err(), "not a book file");
        assert!(place_book(root, "Author/link.lnk", "fp").is_err());
        // A double ending keeps both parts after the counter.
        std::fs::create_dir_all(root.join("A")).unwrap();
        std::fs::write(root.join("A/B.fb2.zip"), b"one").unwrap();
        assert_eq!(place_book(root, "A/B.fb2.zip", "other").unwrap().unwrap(), root.join("A/B (2).fb2.zip"));
        // A file that appears between choosing the name and writing is kept.
        std::fs::write(root.join("A/C.epub"), b"theirs").unwrap();
        assert_eq!(crate::naming::write_new(&root.join("A/C.epub"), b"ours").unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(root.join("A/C.epub")).unwrap(), b"theirs");
        assert!(land_book(root, "A/D.epub", "fp", b"ours").unwrap());
        assert_eq!(std::fs::read(root.join("A/D.epub")).unwrap(), b"ours");
    }

    /// What a `push_book_missing` run did: the books it stored as its own manifest.
    type Stored = std::cell::RefCell<Vec<HashMap<String, crate::cloud::BookRecord>>>;

    #[test]
    fn push_book_missing_uploads_and_records() {
        let uploaded: std::cell::RefCell<Vec<Vec<u8>>> = Default::default();
        let stored: Stored = Default::default();
        let record = book_record("A.epub", 10);
        let ok = push_book_missing(
            "fp",
            &record,
            b"bytes",
            || Ok(None),
            |_, bytes| {
                uploaded.borrow_mut().push(bytes.to_vec());
                Ok(())
            },
            || Ok(None),
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok(())
            },
        )
        .unwrap();
        assert!(ok);
        assert_eq!(uploaded.borrow().as_slice(), [b"bytes".to_vec()]);
        assert_eq!(stored.borrow()[0]["fp"], record);
    }

    #[test]
    fn push_book_missing_keeps_this_devices_earlier_books() {
        let mut mine = HashMap::new();
        mine.insert("old".into(), book_record("O.epub", 1));
        let stored: Stored = Default::default();
        push_book_missing(
            "new",
            &book_record("N.epub", 2),
            b"bytes",
            || Ok(Some(mine.clone())),
            |_, _| Ok(()),
            || Ok(Some(mine.clone())),
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok(())
            },
        )
        .unwrap();
        let stored = stored.borrow();
        assert!(stored[0].contains_key("new") && stored[0].contains_key("old"));
    }

    #[test]
    fn push_book_missing_skips_books_any_device_recorded() {
        let mut theirs = HashMap::new();
        theirs.insert("fp".into(), book_record("A.epub", 10));
        let uploaded = std::cell::Cell::new(false);
        let ok = push_book_missing(
            "fp",
            &book_record("A.epub", 10),
            b"bytes",
            || Ok(Some(theirs.clone())),
            |_, _| {
                uploaded.set(true);
                Ok(())
            },
            || Ok(None),
            |_| Ok(()),
        )
        .unwrap();
        assert!(!ok && !uploaded.get());
    }

    #[test]
    fn push_book_missing_never_overwrites_its_own_unreadable_manifest() {
        let uploaded = std::cell::Cell::new(false);
        let stored: Stored = Default::default();
        let result = push_book_missing(
            "fp",
            &book_record("A.epub", 10),
            b"bytes",
            || Ok(None),
            |_, _| {
                uploaded.set(true);
                Ok(())
            },
            || Err("offline".into()),
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!uploaded.get() && stored.borrow().is_empty(), "nothing moves, nothing is overwritten");
    }

    /// A positions file as a device writes it: (fingerprint, cfi, time, writer).
    fn positions_file(records: &[(&str, &str, u64, &str)]) -> Vec<u8> {
        let positions: serde_json::Map<String, serde_json::Value> = records
            .iter()
            .map(|(fp, cfi, time, by)| {
                (fp.to_string(), serde_json::json!({ "cfi": cfi, "fraction": 0.5, "time": time, "device": by, "deviceId": by }))
            })
            .collect();
        serde_json::json!({ "version": 1, "positions": positions }).to_string().into_bytes()
    }

    #[test]
    fn two_devices_keep_their_own_files_and_see_each_others() {
        let cloud = MemCloud::new(false);
        crate::cloud::push(&cloud, &HashMap::new(), "fp", "epubcfi(/6/2)", 0.2, "PC", "pc").unwrap();
        assert_eq!(cloud.names(), ["positions-pc.json"], "only its own file");
        // The phone read on later.
        cloud.files.lock().unwrap().insert("positions-phone.json".into(), positions_file(&[("fp", "epubcfi(/6/9)", u64::MAX, "phone")]));
        assert_eq!(crate::cloud::pull(&cloud, "fp").unwrap().progress, "epubcfi(/6/9)");
    }

    #[test]
    fn the_device_that_read_there_names_a_shared_place() {
        let cloud = MemCloud::new(false);
        // The PC took the phone's place at startup and passes it on under the phone's name.
        cloud.files.lock().unwrap().insert("positions-a-pc.json".into(), positions_file(&[("fp", "x", 100, "phone")]));
        cloud.files.lock().unwrap().insert("positions-phone.json".into(), positions_file(&[("fp", "x", 100, "phone")]));
        assert_eq!(crate::cloud::pull(&cloud, "fp").unwrap().device_id, "phone");
    }

    #[test]
    fn a_broken_or_old_file_never_hides_the_rest() {
        let cloud = MemCloud::new(false);
        cloud.files.lock().unwrap().insert("positions-broken.json".into(), b"not json".to_vec());
        cloud.files.lock().unwrap().insert("positions-phone.json".into(), positions_file(&[("fp", "new", 200, "phone")]));
        // The shared file of an earlier version: read, but it loses a tie.
        cloud.files.lock().unwrap().insert("sync-state.json".into(), positions_file(&[("fp", "old", 200, "tab"), ("only-old", "o", 5, "tab")]));
        assert_eq!(crate::cloud::pull(&cloud, "fp").unwrap().progress, "new");
        assert_eq!(crate::cloud::pull(&cloud, "only-old").unwrap().progress, "o");
    }

    /// A library with real files on disk and a matching scan.
    fn library_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Store, LibraryBooks, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        let mut entries = Vec::new();
        for (name, content) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            entries.push(crate::library::Entry {
                path: name.to_string(),
                size: content.len() as u64,
                modified: 0,
                fingerprint: crate::fingerprint::fingerprint(&path).unwrap(),
                book: crate::metadata::Book { title: name.to_string(), authors: Vec::new(), series: None, series_index: None, language: None, annotation: None },
                cover: None,
                added: 0,
            });
        }
        let store = Store::load(dir.path().join("breader.json"));
        store
            .update(|s| {
                s.library = Some(root.clone());
                // Hermetic: the OS credential store may hold real logins, so
                // switch the clouds off here — no keyring, no network, ever.
                s.settings.insert(
                    "disabledModules".into(),
                    serde_json::json!([crate::modules::id::DROPBOX, crate::modules::id::GOOGLE_DRIVE]),
                );
            })
            .unwrap();
        let books = LibraryBooks::default();
        books.replace(entries, &root);
        (dir, store, books, root)
    }

    #[test]
    fn missing_books_lists_what_no_cloud_has() {
        // No logins anywhere: nothing can be missing, and nothing touches
        // the network (the credential check fails first).
        let (_dir, store, books, _root) = library_with(&[("A.epub", "aaa")]);
        assert!(missing_cloud_books(&store, &books, &transports()).unwrap().is_empty());
    }

    #[test]
    fn push_book_stays_quiet_without_a_cloud() {
        let (_dir, store, books, root) = library_with(&[("A.epub", "aaa")]);
        let fp = books.snapshot(&store, &root).into_iter().next().unwrap().fingerprint;
        assert_eq!(push_book_to_cloud(&store, &books, &transports(), &fp).unwrap(), false);
    }

    #[test]
    fn pull_all_stays_quiet_without_a_cloud() {
        let (_dir, store, books, _root) = library_with(&[]);
        assert_eq!(pull_all_clouds(&store, &books, &transports()).unwrap(), 0);
    }

    /// A cloud in memory: always connected, its files in a map, or down.
    struct MemCloud {
        files: std::sync::Mutex<HashMap<String, Vec<u8>>>,
        down: bool,
        errors: crate::cloud::ErrorSlot,
        tokens: crate::cloud::TokenCache,
    }

    impl MemCloud {
        fn new(down: bool) -> Self {
            MemCloud { files: Default::default(), down, errors: crate::cloud::ErrorSlot::new(), tokens: crate::cloud::TokenCache::new() }
        }

        fn names(&self) -> Vec<String> {
            let mut names: Vec<_> = self.files.lock().unwrap().keys().cloned().collect();
            names.sort();
            names
        }
    }

    impl CloudStore for MemCloud {
        fn id(&self) -> &'static str {
            "mem"
        }
        fn module(&self) -> &'static str {
            "capability:mem"
        }
        fn token_id(&self) -> &'static str {
            "mem"
        }
        fn label(&self) -> &'static str {
            "Mem"
        }
        fn refresh(&self, _refresh_token: &str) -> Result<(String, u64), SyncError> {
            Ok(("token".into(), 3600))
        }
        fn connected(&self) -> bool {
            true
        }
        fn access_token(&self) -> Result<String, SyncError> {
            if self.down { Err(SyncError::offline("Mem is down".into())) } else { Ok("token".into()) }
        }
        fn get(&self, _access: &str, name: &str) -> Result<Option<Vec<u8>>, SyncError> {
            Ok(self.files.lock().unwrap().get(name).cloned())
        }
        fn put(&self, _access: &str, name: &str, bytes: &[u8]) -> Result<(), SyncError> {
            self.files.lock().unwrap().insert(name.into(), bytes.to_vec());
            Ok(())
        }
        fn list(&self, _access: &str, prefix: &str) -> Result<Vec<String>, SyncError> {
            Ok(self.names().into_iter().filter(|n| n.starts_with(prefix)).collect())
        }
        fn delete(&self, _access: &str, name: &str) -> Result<(), SyncError> {
            self.files.lock().unwrap().remove(name);
            Ok(())
        }
        fn errors(&self) -> &crate::cloud::ErrorSlot {
            &self.errors
        }
        fn tokens(&self) -> &crate::cloud::TokenCache {
            &self.tokens
        }
    }

    #[test]
    fn a_book_travels_through_a_cloud_to_another_library() {
        let cloud = MemCloud::new(false);
        let via = CloudTransport(&cloud);
        let (_dir, store, books, _root) = library_with(&[("A/Book.epub", "the book")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        assert!(push_book_to_cloud(&store, &books, &[&via], &fp).unwrap());
        let names = cloud.names();
        assert_eq!(names[0], format!("book-{fp}-A_Book.epub"));
        assert!(names[1].starts_with("books-") && names.len() == 2, "{names:?}");
        assert!(!push_book_to_cloud(&store, &books, &[&via], &fp).unwrap(), "already there");
        // Another device: offered, then downloaded under its name, same bytes.
        let (_dir2, other, other_books, other_root) = library_with(&[]);
        let missing = missing_cloud_books(&other, &other_books, &[&via]).unwrap();
        assert_eq!((missing.len(), missing[0].name.as_str()), (1, "A/Book.epub"));
        assert!(pull_book_from_cloud(&other, &[&via], &fp).unwrap());
        assert_eq!(std::fs::read(other_root.join("A/Book.epub")).unwrap(), b"the book");
    }

    #[test]
    fn a_cloud_that_is_down_never_keeps_a_book_from_the_others() {
        let down = MemCloud::new(true);
        let up = MemCloud::new(false);
        let (_dir, store, books, _root) = library_with(&[("Book.epub", "bytes")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        assert!(push_book_to_cloud(&store, &books, &[&CloudTransport(&down), &CloudTransport(&up)], &fp).unwrap());
        assert_eq!(up.names().len(), 2);
        assert_eq!(down.errors.last().as_deref(), Some("Mem is down"), "the settings show why");
        // Only the down cloud: its failure is the answer.
        assert!(push_book_to_cloud(&store, &books, &[&CloudTransport(&down)], &fp).is_err());
    }

    #[test]
    fn positions_round_trip_through_a_cloud() {
        let cloud = MemCloud::new(false);
        let local = HashMap::new();
        crate::cloud::push(&cloud, &local, "fp", "epubcfi(/6/4)", 0.4, "Phone", "phone-id").unwrap();
        let progress = crate::cloud::pull(&cloud, "fp").unwrap();
        assert_eq!((progress.progress.as_str(), progress.device_id.as_str()), ("epubcfi(/6/4)", "phone-id"));
        // The startup pull gives a library book never opened here the cloud's place.
        let (_dir, store, books, _root) = library_with(&[("Book.epub", "bytes")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        crate::cloud::push(&cloud, &local, &fp, "epubcfi(/6/8)", 0.7, "Phone", "phone-id").unwrap();
        assert_eq!(pull_all_clouds(&store, &books, &[&CloudTransport(&cloud)]).unwrap(), 1);
        assert_eq!(store.read(|s| s.positions[&fp].cfi.clone()), "epubcfi(/6/8)");
    }

    /// A library book as the scan lists it, for the reconcile rules.
    fn entry(fingerprint: &str, added: u64) -> crate::library::Entry {
        crate::library::Entry {
            path: format!("{fingerprint}.epub"),
            size: 1,
            modified: 0,
            fingerprint: fingerprint.into(),
            book: crate::metadata::Book { title: fingerprint.into(), authors: Vec::new(), series: None, series_index: None, language: None, annotation: None },
            cover: None,
            added,
        }
    }

    #[test]
    fn the_library_and_the_clouds_are_brought_together() {
        let record = |added: u64, removed: Option<u64>| crate::cloud::BookRecord { added, removed, ..book_record("x.epub", 1) };
        let mut a = HashMap::new();
        a.insert("both".to_string(), record(1, None));
        a.insert("gone-elsewhere".to_string(), record(1, Some(50)));
        a.insert("came-back".to_string(), record(1, Some(50)));
        let mut b = a.clone();
        b.remove("both");
        let per_cloud = vec![a.clone(), b];
        let latest = super::latest_of(&per_cloud).unwrap();
        let library = [entry("both", 10), entry("new", 10), entry("gone-elsewhere", 10), entry("came-back", 90)];
        let (uploads, moved_out) = super::engine::sort_library(&library, &per_cloud, &latest, 0);
        // "both" is missing from the second cloud; "came-back" was added again after its removal.
        assert_eq!(uploads, ["both", "new", "came-back"]);
        assert_eq!(moved_out.iter().map(|e| e.fingerprint.as_str()).collect::<Vec<_>>(), ["gone-elsewhere"]);
        // A device whose first scan came after the removal keeps its old copy.
        let (_, moved_out) = super::engine::sort_library(&library, &per_cloud, &latest, 60);
        assert!(moved_out.is_empty());
    }

    #[test]
    fn a_restored_book_leaves_removed_books_and_comes_down_again() {
        let cloud = MemCloud::new(false);
        let via = CloudTransport(&cloud);
        let (_dir, store, books, root) = library_with(&[("Book.epub", "bytes")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        assert!(push_book_to_cloud(&store, &books, &[&via], &fp).unwrap());
        std::fs::remove_file(root.join("Book.epub")).unwrap();
        store.update(|s| { s.removed_here.insert(fp.clone()); }).unwrap();
        super::restore_books(&store, &[fp.clone()]).unwrap();
        assert!(store.read(|s| !s.removed_here.contains(&fp)), "no longer removed here");
        super::engine::run_due(&store, &books, &[&via], 0);
        assert!(store.read(|s| s.outbox.is_empty()));
        assert!(root.join("Book.epub").exists(), "the file is back in the library");
    }

    #[test]
    fn a_removal_waits_for_a_cloud_that_cannot_be_read() {
        let down = MemCloud::new(true);
        let (_dir, store, books, _root) = library_with(&[]);
        store
            .update(|s| {
                s.removed_here.insert("fp".into());
                super::engine::queue(&mut s.outbox, Job::Remove { fingerprint: "fp".into() });
            })
            .unwrap();
        super::engine::run_due(&store, &books, &[&CloudTransport(&down)], 0);
        assert_eq!(store.read(|s| s.outbox[0].attempts), 1, "retried, not done");
        assert!(store.read(|s| s.removed_here.contains("fp")), "and never downloaded back meanwhile");
    }

    #[test]
    fn a_book_removed_from_the_clouds_is_gone_for_every_device() {
        let cloud = MemCloud::new(false);
        let via = CloudTransport(&cloud);
        let (_dir, store, books, _root) = library_with(&[("Book.epub", "bytes")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        assert!(push_book_to_cloud(&store, &books, &[&via], &fp).unwrap());
        store.update(|s| super::engine::queue(&mut s.outbox, Job::Remove { fingerprint: fp.clone() })).unwrap();
        super::engine::run_due(&store, &books, &[&via], 0);
        assert!(store.read(|s| s.outbox.is_empty()));
        assert!(!cloud.names().iter().any(|n| n.starts_with("book-")), "the file went");
        assert!(store.read(|s| !s.removed_here.contains(&fp)), "Removed books forgets it once every cloud has the removal");
        let latest = super::latest_books(&store, &[&via]).unwrap();
        assert!(latest[&fp].removed.is_some(), "and every device learns of it");
        // Another device lists it neither as missing nor as Removed books.
        let (_dir2, other, other_books, _r) = library_with(&[]);
        assert!(missing_cloud_books(&other, &other_books, &[&via]).unwrap().is_empty());
    }

    #[test]
    fn pull_book_stays_quiet_without_a_cloud() {
        let (_dir, store, _books, _root) = library_with(&[]);
        assert_eq!(pull_book_from_cloud(&store, &transports(), "fp").unwrap(), false);
    }

    #[test]
    fn google_needs_its_module_and_the_intent() {
        let (_dir, store) = synced_store(&[]);
        assert!(store.read(|s| GOOGLE.enabled(s)));
        let (_dir, store) = synced_store(&[crate::modules::id::GOOGLE_DRIVE]);
        assert!(!store.read(|s| GOOGLE.enabled(s)));
        let (_dir, store) = synced_store(&[crate::modules::id::PROGRESS_SYNC]);
        assert!(!store.read(|s| GOOGLE.enabled(s)), "the intent switch gates every transport");
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

    /// Queues the book's position for these transports, as a window does.
    fn queue_position(store: &Store, ids: &[&str]) {
        store
            .update(|s| {
                for id in ids {
                    let job = Job::Position { transport: id.to_string(), fingerprint: "fp".into(), document: "doc".into(), cfi: "a".into(), fraction: 0.5 };
                    super::engine::queue(&mut s.outbox, job);
                }
            })
            .unwrap();
    }

    /// The transports still owed a position.
    fn owed(store: &Store) -> Vec<String> {
        store.read(|s| {
            s.outbox
                .iter()
                .filter_map(|p| match &p.job {
                    Job::Position { transport, .. } => Some(transport.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    #[test]
    fn disabled_transports_are_skipped() {
        let (_dir, store) = synced_store(&[]);
        let off = Fake::of("off", Ok(Some(server("b", "phone", 2))), Ok(())).off();
        let remote = pull_merged(&store, &as_transports(&[&off]), &book());
        assert!(remote.is_none());
        queue_position(&store, &["off"]);
        super::engine::run_due(&store, &LibraryBooks::default(), &as_transports(&[&off]), 0);
        assert!(off.calls.borrow().is_empty());
        assert!(owed(&store).is_empty(), "nothing to do for a transport that is off");
    }

    #[test]
    fn disconnected_transports_are_skipped_not_failed() {
        let (_dir, store) = synced_store(&[]);
        let kosync = Fake::of("kosync", Ok(None), Ok(()));
        let cloud = Fake::of("dropbox", Ok(Some(server("b", "phone", 2))), Err("not connected".into())).disconnected();
        assert!(pull_merged(&store, &as_transports(&[&kosync, &cloud]), &book()).is_none());
        queue_position(&store, &["kosync", "dropbox"]);
        super::engine::run_due(&store, &LibraryBooks::default(), &as_transports(&[&kosync, &cloud]), 0);
        assert!(owed(&store).is_empty(), "a cloud without a login is never retried");
        assert!(cloud.calls.borrow().is_empty());
    }

    #[test]
    fn a_failing_transport_is_retried_alone_later() {
        let (_dir, store) = synced_store(&[]);
        let ok = Fake::of("ok", Ok(None), Ok(()));
        let failing = Fake::of("failing", Ok(None), Err("down".into()));
        let both = as_transports(&[&ok, &failing]);
        queue_position(&store, &["ok", "failing"]);
        super::engine::run_due(&store, &LibraryBooks::default(), &both, 1_000);
        assert_eq!(owed(&store), ["failing"]);
        // Not due yet: nothing is tried.
        super::engine::run_due(&store, &LibraryBooks::default(), &both, 2_000);
        assert_eq!(failing.calls.borrow().len(), 1);
        // Due again later: only the failing one goes.
        super::engine::run_due(&store, &LibraryBooks::default(), &both, 1_000_000);
        assert_eq!(ok.calls.borrow().as_slice(), ["push"], "a clean transport is not sent the same place again");
        assert_eq!(failing.calls.borrow().as_slice(), ["push", "push"]);
        assert_eq!(store.read(|s| s.outbox[0].attempts), 2);
    }

    #[test]
    fn book_files_wait_and_arrive_through_the_outbox() {
        let cloud = MemCloud::new(false);
        let via = CloudTransport(&cloud);
        let (_dir, store, books, _root) = library_with(&[("Book.epub", "bytes")]);
        let fp = books.snapshot(&store, &library_root(&store).unwrap()).into_iter().next().unwrap().fingerprint;
        store.update(|s| super::engine::queue(&mut s.outbox, Job::Upload { fingerprint: fp.clone() })).unwrap();
        super::engine::run_due(&store, &books, &[&via], 0);
        assert_eq!(cloud.names().len(), 2, "uploaded");
        assert!(store.read(|s| s.outbox.is_empty()));
        // Another device asks for it: it lands and is reported as arrived.
        let (_dir2, other, other_books, other_root) = library_with(&[]);
        other.update(|s| super::engine::queue(&mut s.outbox, Job::Download { fingerprint: fp.clone() })).unwrap();
        assert_eq!(super::engine::run_due(&other, &other_books, &[&via], 0).arrived, [fp]);
        assert!(other_root.join("Book.epub").is_file());
    }

}
