//! Reading progress sync (KOSync): what the server says about a book.

use crate::catalog;
use crate::clock::now_ms;
use crate::cloud::RemoteRecord;
use crate::fingerprint;
use crate::formats;
use crate::kosync;
use crate::library_cmds::{library_root, LibraryBooks};
use crate::modules;
use crate::platform::{Active, Platform};
use crate::store::{self, Position, Store};
use crate::windows::ReaderWindows;
use crate::CmdResult;
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
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
    /// Whether the transport can reach its store: a cloud needs a login (a
    /// refresh token is kept); KOSync needs nothing more than being enabled.
    /// A disconnected cloud is skipped silently, never reported as failed.
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
    /// Downloads a book file by its cloud name; `None` when the cloud never
    /// saw it.
    fn pull_book(&self, remote_name: &str) -> Result<Option<Vec<u8>>, String> {
        let _ = remote_name;
        Ok(None)
    }
    /// Uploads a book file the cloud lacks, and records it in the cloud
    /// manifest; true when the cloud gained the book.
    fn push_book(&self, fingerprint: &str, record: &crate::cloud::BookRecord, bytes: &[u8]) -> Result<bool, String> {
        let _ = (fingerprint, record, bytes);
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

    fn connected(&self) -> bool {
        crate::dropbox::is_connected()
    }

    fn keeps_books(&self) -> bool {
        true
    }

    fn books(&self) -> Option<HashMap<String, crate::cloud::BookRecord>> {
        crate::dropbox::book_manifest()
    }

    fn pull_book(&self, remote_name: &str) -> Result<Option<Vec<u8>>, String> {
        crate::dropbox::download_book(remote_name)
    }

    fn push_book(&self, fingerprint: &str, record: &crate::cloud::BookRecord, bytes: &[u8]) -> Result<bool, String> {
        crate::dropbox::push_book(fingerprint, record, bytes)
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

    fn connected(&self) -> bool {
        crate::google::is_connected()
    }

    fn keeps_books(&self) -> bool {
        true
    }

    fn books(&self) -> Option<HashMap<String, crate::cloud::BookRecord>> {
        crate::google::book_manifest()
    }

    fn pull_book(&self, remote_name: &str) -> Result<Option<Vec<u8>>, String> {
        crate::google::download_book(remote_name)
    }

    fn push_book(&self, fingerprint: &str, record: &crate::cloud::BookRecord, bytes: &[u8]) -> Result<bool, String> {
        crate::google::push_book(fingerprint, record, bytes)
    }
}

static KOSYNC: Kosync = Kosync;
static DROPBOX: Dropbox = Dropbox;
static GOOGLE: Google = Google;

/// Every transport, in merge order (first wins timestamp ties).
fn transports() -> [&'static dyn Transport; 3] {
    [&KOSYNC, &DROPBOX, &GOOGLE]
}

/// Whether a transport takes part right now: its modules are on and it is
/// connected. Anything else is skipped silently, not counted as a failure.
fn usable(store: &Store, t: &dyn Transport) -> bool {
    store.read(|s| t.enabled(s)) && t.connected()
}

/// The usable transports that keep book files.
fn book_stores(store: &Store) -> Vec<&'static dyn Transport> {
    transports().into_iter().filter(|t| t.keeps_books() && usable(store, *t)).collect()
}

/// The newest remote position across the transports. A transport that fails
/// reports nothing instead of failing the pull, so a dead transport never
/// blocks a live one.
fn pull_merged(store: &Store, transports: &[&dyn Transport], book: &BookKey) -> Option<kosync::Progress> {
    let mut remote = None;
    for t in transports {
        if !usable(store, *t) {
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
        if record.device_id == own_device {
            continue;
        }
        // A new entry only for a library book, and only with a real place to take.
        if !local.contains_key(fingerprint) && (!library.contains(fingerprint) || record.time == 0) {
            continue;
        }
        let here = local.entry(fingerprint.clone()).or_insert(Position { cfi: String::new(), fraction: 0.0, time: 0 });
        if record.time > here.time {
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
/// skipped like in a pull. Once per start of the app, however many windows
/// ask: later calls report nothing changed.
#[tauri::command(async)]
pub(crate) fn sync_pull_all(store: State<Store>, books: State<LibraryBooks>) -> CmdResult<usize> {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(0);
    }
    pull_all_clouds(&store, &books)
}

/// Merges every usable cloud's positions file into the local positions, for
/// the whole library; returns how many books changed.
fn pull_all_clouds(store: &Store, books: &LibraryBooks) -> CmdResult<usize> {
    let library: BTreeSet<String> = match library_root(store) {
        Ok(root) => books.snapshot(store, &root).into_iter().map(|b| b.fingerprint).collect(),
        Err(_) => BTreeSet::new(),
    };
    merge_cloud_positions(store, &library)
}

/// Merges every usable cloud's positions file into the local positions;
/// books in `library` never opened here take the cloud's place too.
/// Returns how many books changed.
fn merge_cloud_positions(store: &Store, library: &BTreeSet<String>) -> CmdResult<usize> {
    let device = device_id(store)?;
    let mut updated = 0;
    for t in transports() {
        if !usable(store, t) {
            continue;
        }
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

/// Uploads a reading book's file to every connected cloud that lacks it.
/// Best-effort and silent: without a login there is nothing to push to, and
/// books the library scan never saw are skipped. True when at least one
/// cloud gained the book.
#[tauri::command(async)]
pub(crate) fn sync_push_book(store: State<Store>, books: State<LibraryBooks>, fingerprint: String) -> CmdResult<bool> {
    push_book_to_cloud(&store, &books, &fingerprint)
}

fn push_book_to_cloud(store: &Store, books: &LibraryBooks, fingerprint: &str) -> CmdResult<bool> {
    if !book_transfer_allowed(store) {
        return Ok(false);
    }
    let clouds = book_stores(store);
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
    };
    let mut pushed = false;
    let mut failed = None;
    // One cloud down never keeps the book from the others.
    for t in clouds {
        match t.push_book(fingerprint, &record, &bytes) {
            Ok(gained) => pushed |= gained,
            Err(e) => failed = Some(e),
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

/// The cloud books the library lacks, first cloud wins on duplicates: same
/// fingerprint means same bytes, so one entry is enough.
fn collect_missing(manifests: &[&HashMap<String, crate::cloud::BookRecord>], local: &BTreeSet<String>) -> Vec<MissingBook> {
    let mut seen = BTreeSet::new();
    let mut missing = Vec::new();
    for books in manifests {
        for (fingerprint, record) in books.iter() {
            if local.contains(fingerprint.as_str()) || !seen.insert(fingerprint.clone()) {
                continue;
            }
            missing.push(MissingBook { fingerprint: fingerprint.clone(), name: record.name.clone(), size: record.size });
        }
    }
    missing
}

/// The cloud books the library lacks, for the download offer. Silent like a
/// pull: transports that are off or disconnected report nothing.
#[tauri::command(async)]
pub(crate) fn sync_missing_books(store: State<Store>, books: State<LibraryBooks>) -> CmdResult<Vec<MissingBook>> {
    missing_cloud_books(&store, &books)
}

fn missing_cloud_books(store: &Store, books: &LibraryBooks) -> CmdResult<Vec<MissingBook>> {
    let root = library_root(store)?;
    let local: BTreeSet<String> = books.snapshot(store, &root).into_iter().map(|b| b.fingerprint).collect();
    let mut manifests = Vec::new();
    for t in book_stores(store) {
        if let Some(mut books) = t.books() {
            // Only books this device would open: a book file, its format on.
            books.retain(|_, record| book_format_allowed(store, &record.name).is_ok());
            manifests.push(books);
        }
    }
    let refs: Vec<&HashMap<String, crate::cloud::BookRecord>> = manifests.iter().collect();
    Ok(collect_missing(&refs, &local))
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
/// book already holds it — then ` (2)`, like Organize. `None` when the
/// library already has this very book there. Names that aren't book files
/// the app opens never land, whatever the cloud claims.
fn place_book(root: &Path, name: &str, fingerprint: &str) -> CmdResult<Option<PathBuf>> {
    let rel = safe_relative(name)?;
    let file_name = rel.file_name().and_then(|s| s.to_str()).unwrap_or_default();
    let Some((stem, _)) = formats::split(file_name) else {
        return Err("The cloud name is not a book file.".into());
    };
    let ext = &file_name[stem.len()..];
    let dest = root.join(&rel);
    if !dest.exists() {
        return Ok(Some(dest));
    }
    if fingerprint::fingerprint(&dest).map_err(|e| e.to_string())? == fingerprint {
        return Ok(None);
    }
    let parent = rel.parent().unwrap_or_else(|| Path::new(""));
    for n in 2..1000 {
        let candidate = root.join(parent).join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return Ok(Some(candidate));
        }
    }
    Err("No free name for the downloaded book.".into())
}

/// Writes a downloaded book to a file that must not exist yet, so nothing in
/// the library is ever overwritten; a write that fails leaves no half book.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes).inspect_err(|_| {
        let _ = std::fs::remove_file(path);
    })
}

/// Places a verified download in the library: picks its name and writes it,
/// trying the next free name when another file appears there meanwhile.
/// False when the library already holds the book.
fn land_book(root: &Path, name: &str, fingerprint: &str, bytes: &[u8]) -> CmdResult<bool> {
    for _ in 0..3 {
        let Some(dest) = place_book(root, name, fingerprint)? else { return Ok(false) };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        match write_new(&dest, bytes) {
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

/// Downloads a book file the library lacks, verifying it against the
/// manifest hash, and places it in the library. Best-effort and silent like
/// an upload.
#[tauri::command(async)]
pub(crate) fn sync_pull_book(store: State<Store>, fingerprint: String) -> CmdResult<bool> {
    pull_book_from_cloud(&store, &fingerprint)
}

fn pull_book_from_cloud(store: &Store, fingerprint: &str) -> CmdResult<bool> {
    if !book_transfer_allowed(store) {
        return Ok(false);
    }
    let root = library_root(store)?;
    let mut failed = None;
    // A cloud that fails, or sends a copy that fails its hash, hands over to the next.
    for t in book_stores(store) {
        let Some(manifest) = t.books() else { continue };
        let Some(record) = manifest.get(fingerprint) else { continue };
        if let Err(e) = book_format_allowed(store, &record.name) {
            failed = Some(e);
            continue;
        }
        let remote_name = crate::cloud::remote_book_name(fingerprint, &record.name);
        let bytes = match t.pull_book(&remote_name) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(e) => {
                failed = Some(e);
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
            // The new book opens where the other device is, not with a jump offer.
            let _ = merge_cloud_positions(store, &[fingerprint.to_string()].into_iter().collect());
        }
        return Ok(landed);
    }
    failed.map_or(Ok(false), Err)
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
        .filter(|t| usable(store, **t))
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
    use super::{book_format_allowed, book_transfer_allowed, book_transfer_gate, collect_missing, compare_positions, land_book, merge_remote, write_new, missing_cloud_books, place_book, pull_all_clouds, pull_book_from_cloud, pull_merged, push_all, push_book_to_cloud, safe_relative, sync_source, BookKey, Dropbox, Google, Kosync, Transport};
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
        crate::cloud::BookRecord { name: name.into(), size, sha256: "abc".into() }
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
        let local: std::collections::BTreeSet<String> = ["here".into()].into_iter().collect();
        let missing = collect_missing(&[&a, &b], &local);
        assert_eq!(missing.len(), 2);
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
        let (_dir, store) = synced_store(&["format:fb2"]);
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
        assert_eq!(write_new(&root.join("A/C.epub"), b"ours").unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(root.join("A/C.epub")).unwrap(), b"theirs");
        assert!(land_book(root, "A/D.epub", "fp", b"ours").unwrap());
        assert_eq!(std::fs::read(root.join("A/D.epub")).unwrap(), b"ours");
    }

    #[test]
    fn push_book_missing_uploads_and_records() {
        use std::cell::RefCell;
        let uploaded: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
        let stored: RefCell<Vec<HashMap<String, crate::cloud::BookRecord>>> = RefCell::new(Vec::new());
        let record = book_record("A.epub", 10);
        let ok = push_book_missing(
            "fp",
            &record,
            b"bytes",
            || Ok(None),
            |_, bytes| {
                uploaded.borrow_mut().push(bytes.to_vec());
                Ok::<(), String>(())
            },
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok::<(), String>(())
            },
        )
        .unwrap();
        assert!(ok);
        assert_eq!(uploaded.borrow().as_slice(), [b"bytes".to_vec()]);
        assert_eq!(stored.borrow().len(), 1);
        assert_eq!(stored.borrow()[0]["fp"], record);
    }

    #[test]
    fn push_book_missing_merges_with_a_fresh_manifest() {
        use std::cell::{Cell, RefCell};
        let mut other = HashMap::new();
        other.insert("old".into(), book_record("O.epub", 1));
        let calls = Cell::new(0);
        let stored: RefCell<Vec<HashMap<String, crate::cloud::BookRecord>>> = RefCell::new(Vec::new());
        let ok = push_book_missing(
            "new",
            &book_record("N.epub", 2),
            b"bytes",
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Ok(Some(HashMap::new()))
                } else {
                    Ok(Some(other.clone()))
                }
            },
            |_, _| Ok::<(), String>(()),
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok::<(), String>(())
            },
        )
        .unwrap();
        assert!(ok);
        let stored = stored.borrow();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].contains_key("new") && stored[0].contains_key("old"), "both uploads survive");
    }

    #[test]
    fn push_book_missing_skips_recorded_books() {
        use std::cell::RefCell;
        let mut manifest = HashMap::new();
        manifest.insert("fp".into(), book_record("A.epub", 10));
        let uploaded: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
        let ok = push_book_missing(
            "fp",
            &book_record("A.epub", 10),
            b"bytes",
            || Ok(Some(manifest.clone())),
            |_, bytes| {
                uploaded.borrow_mut().push(bytes.to_vec());
                Ok::<(), String>(())
            },
            |_| Ok::<(), String>(()),
        )
        .unwrap();
        assert!(!ok);
        assert!(uploaded.borrow().is_empty());
    }

    #[test]
    fn push_book_missing_never_overwrites_an_unreadable_manifest() {
        use std::cell::{Cell, RefCell};
        let calls = Cell::new(0);
        let stored: RefCell<Vec<HashMap<String, crate::cloud::BookRecord>>> = RefCell::new(Vec::new());
        // The first read finds nothing, the re-read before writing fails.
        let result = push_book_missing(
            "fp",
            &book_record("A.epub", 10),
            b"bytes",
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Ok(None)
                } else {
                    Err("offline".into())
                }
            },
            |_, _| Ok::<(), String>(()),
            |books| {
                stored.borrow_mut().push(books.clone());
                Ok::<(), String>(())
            },
        );
        assert!(result.is_err());
        assert!(stored.borrow().is_empty(), "the manifest is left alone");
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
                    serde_json::json!(["capability:dropbox", "capability:googledrive"]),
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
        assert!(missing_cloud_books(&store, &books).unwrap().is_empty());
    }

    #[test]
    fn push_book_stays_quiet_without_a_cloud() {
        let (_dir, store, books, root) = library_with(&[("A.epub", "aaa")]);
        let fp = books.snapshot(&store, &root).into_iter().next().unwrap().fingerprint;
        assert_eq!(push_book_to_cloud(&store, &books, &fp).unwrap(), false);
    }

    #[test]
    fn pull_all_stays_quiet_without_a_cloud() {
        let (_dir, store, books, _root) = library_with(&[]);
        assert_eq!(pull_all_clouds(&store, &books).unwrap(), 0);
    }

    #[test]
    fn pull_book_stays_quiet_without_a_cloud() {
        let (_dir, store, _books, _root) = library_with(&[]);
        assert_eq!(pull_book_from_cloud(&store, "fp").unwrap(), false);
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
    fn disconnected_transports_are_skipped_not_failed() {
        let (_dir, store) = synced_store(&[]);
        let kosync = Fake::of("kosync", Ok(None), Ok(()));
        let cloud = Fake::of("dropbox", Ok(Some(server("b", "phone", 2))), Err("not connected".into())).disconnected();
        assert!(pull_merged(&store, &as_transports(&[&kosync, &cloud]), &book()).is_none());
        let reports = push_all(&store, &as_transports(&[&kosync, &cloud]), &None, &book(), "a", 0.5);
        assert_eq!(reports.len(), 1, "no report means no retry for the cloud");
        assert_eq!((reports[0].transport.as_str(), reports[0].error.as_deref()), ("kosync", None));
        assert!(cloud.calls.borrow().is_empty());
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
