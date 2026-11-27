//! The library folder: what it holds, and moving books into it.

use crate::formats;
use crate::library;
use crate::metadata;
use crate::modules;
use crate::naming;
use crate::paths;
use crate::platform::{Active, Platform};
use crate::store::Store;
use crate::system_cmds::Originals;
use crate::windows::{saving_window, show_book, ReaderWindows};
use crate::CmdResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State, Window, ipc::Response};

/// The books found by the last scan, with indexes so the hot paths (recent
/// books, covers, adding a book) never copy the whole list. The frontend
/// names books by their key (the path inside the library), which is only
/// accepted when the scan found it, so the frontend still cannot make the
/// app open arbitrary files.
#[derive(Default)]
pub(crate) struct LibraryBooks(Mutex<Scan>);

#[derive(Default)]
struct Scan {
    entries: Vec<library::Entry>,
    /// Fingerprint to an entry, for duplicate checks without copying the list.
    by_fingerprint: HashMap<String, usize>,
    /// Library-relative paths of books with covers, by fingerprint.
    covers: HashMap<String, String>,
    /// What was on disk at the last scan: (fingerprint, absolute path).
    /// Trusted for `exists` until the next scan, which runs on every visit
    /// to the library screen.
    present: HashSet<(String, PathBuf)>,
    /// The folder the scan ran in (for `forget`, after entries moved).
    root: PathBuf,
}

impl Scan {
    fn build(entries: Vec<library::Entry>, root: &Path) -> Self {
        let mut scan = Scan { entries, root: root.to_path_buf(), ..Default::default() };
        scan.reindex();
        scan
    }

    fn reindex(&mut self) {
        self.by_fingerprint.clear();
        self.covers.clear();
        self.present.clear();
        for (index, entry) in self.entries.iter().enumerate() {
            self.by_fingerprint.entry(entry.fingerprint.clone()).or_insert(index);
            if entry.cover.is_some() {
                self.covers.insert(entry.fingerprint.clone(), entry.path.clone());
            }
            self.present.insert((entry.fingerprint.clone(), entry.absolute(&self.root)));
        }
    }
}

impl LibraryBooks {
    pub(crate) fn entry(&self, key: &str) -> CmdResult<library::Entry> {
        let books = self.0.lock().unwrap();
        books.entries.iter().find(|b| b.path == key).cloned().ok_or_else(|| "That book is no longer in the library".into())
    }

    pub(crate) fn path_of(&self, store: &Store, key: &str) -> CmdResult<PathBuf> {
        Ok(self.entry(key)?.absolute(&library_root(store)?))
    }

    /// Takes a deleted book out of the list.
    pub(crate) fn forget(&self, key: &str) {
        let mut scan = self.0.lock().unwrap();
        scan.entries.retain(|b| b.path != key);
        scan.reindex();
    }

    /// The last scan's books; before any scan in this run, the cached ones.
    /// Only Organize needs the whole list; the hot paths use the indexes.
    pub(crate) fn snapshot(&self, store: &Store, root: &Path) -> Vec<library::Entry> {
        let books = self.0.lock().unwrap();
        if books.entries.is_empty() { library_cache(store).load(root) } else { books.entries.clone() }
    }

    /// Remembers a scan and rebuilds the indexes.
    pub(crate) fn replace(&self, entries: Vec<library::Entry>, root: &Path) {
        *self.0.lock().unwrap() = Scan::build(entries, root);
    }

    /// Forgets the last scan's books: they belonged to the old folder.
    pub(crate) fn clear(&self) {
        *self.0.lock().unwrap() = Scan::default();
    }

    /// Whether the scan is from this run (past the first scan, memory serves).
    pub(crate) fn is_scanned(&self) -> bool {
        !self.0.lock().unwrap().entries.is_empty()
    }

    /// The entry with `key`, without copying the list.
    pub(crate) fn find(&self, key: &str) -> Option<library::Entry> {
        self.0.lock().unwrap().entries.iter().find(|b| b.path == key).cloned()
    }

    /// The entry with a fingerprint, in the library or its disk cache.
    pub(crate) fn find_by_fingerprint(&self, store: &Store, fingerprint: &str) -> Option<library::Entry> {
        if let Some(entry) = self.0.lock().unwrap().entries.iter().find(|b| b.fingerprint == fingerprint).cloned() {
            return Some(entry);
        }
        let root = library_root(store).ok()?;
        self.cached(store, &root).into_iter().find(|b| b.fingerprint == fingerprint)
    }

    /// The last scan's books from the disk cache, for the pre-scan run.
    pub(crate) fn cached(&self, store: &Store, root: &Path) -> Vec<library::Entry> {
        library_cache(store).load(root)
    }

    /// Library-relative paths of books with covers, by fingerprint: from the
    /// scan, or from the disk cache before the first scan in the run.
    pub(crate) fn covers(&self, store: &Store, root: &Path) -> HashMap<String, String> {
        let scan = self.0.lock().unwrap();
        if !scan.entries.is_empty() {
            return scan.covers.clone();
        }
        drop(scan);
        Self::cached_map(&library_cache(store).load(root))
    }

    fn cached_map(entries: &[library::Entry]) -> HashMap<String, String> {
        entries.iter().filter(|e| e.cover.is_some()).map(|e| (e.fingerprint.clone(), e.path.clone())).collect()
    }

    /// The library key already holding `fingerprint` (a copy on disk), if any:
    /// from the scan, or from the disk cache before the first scan in the run.
    pub(crate) fn duplicate(&self, store: &Store, root: &Path, fingerprint: &str) -> Option<String> {
        let scan = self.0.lock().unwrap();
        if !scan.entries.is_empty() {
            let entry = scan.by_fingerprint.get(fingerprint).and_then(|&index| scan.entries.get(index))?;
            return entry.absolute(root).is_file().then(|| entry.path.clone());
        }
        drop(scan);
        self.cached(store, root)
            .into_iter()
            .find(|e| e.fingerprint == fingerprint && e.absolute(root).is_file())
            .map(|e| e.path)
    }

    /// Whether the scan saw `fingerprint` at `path` (checked live otherwise).
    pub(crate) fn is_present(&self, fingerprint: &str, path: &Path) -> bool {
        self.0.lock().unwrap().present.contains(&(fingerprint.to_string(), path.to_path_buf()))
    }
}

pub(crate) fn library_root(store: &Store) -> CmdResult<PathBuf> {
    store.read(|s| s.library.clone()).ok_or_else(|| "No library folder is set".into())
}

/// The app's cache folder, next to its settings.
pub(crate) const CACHE_DIR: &str = "breader-cache";

fn library_cache(store: &Store) -> library::Cache {
    library::Cache::new(store.dir().join(CACHE_DIR))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryInfo {
    /// The library folder, once chosen.
    folder: Option<String>,
    /// What the first-run screen offers.
    suggested: String,
}

#[tauri::command(async)]
pub(crate) fn library_info(app: AppHandle, store: State<Store>) -> CmdResult<LibraryInfo> {
    Ok(LibraryInfo {
        folder: store.read(|s| s.library.as_ref().map(|p| p.display().to_string())),
        suggested: Active::suggested_library(&app)?.display().to_string(),
    })
}

/// Sets the library folder: the suggested one, or one the reader picks.
/// Returns the folder, or `None` when the picker was cancelled.
#[tauri::command(async)]
pub(crate) fn set_library(window: Window, store: State<Store>, books: State<LibraryBooks>, pick: bool) -> CmdResult<Option<String>> {
    let Some(folder) = Active::chosen_library(&window, pick)? else { return Ok(None) };
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let folder = std::path::absolute(&folder).unwrap_or(folder);
    store.update(|s| s.library = Some(folder.clone())).map_err(|e| e.to_string())?;
    // The last scan's keys belong to the old folder.
    books.clear();
    let _ = window.emit("library-changed", ());
    Ok(Some(folder.display().to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryBook {
    key: String,
    /// The book's content identity, for sync and details from any window.
    fingerprint: String,
    title: String,
    /// "First Last", one per author.
    authors: Vec<String>,
    /// "Last First" of the first author, for sorting.
    author_sort: String,
    series: Option<String>,
    series_index: Option<String>,
    format: &'static str,
    /// Share read, if a position is saved.
    fraction: Option<f64>,
    /// When the book came into the library (ms), for "recently added".
    added: u64,
    has_cover: bool,
    /// Its format is turned off in Modules: shown, but it won't open.
    disabled: bool,
}

/// Scans the library folder (cheap after the first time: see library::scan).
#[tauri::command(async)]
pub(crate) fn scan_library(
    app: AppHandle,
    store: State<Store>,
    books: State<LibraryBooks>,
    engine: State<crate::sync_cmds::SyncEngine>,
) -> CmdResult<Vec<LibraryBook>> {
    let root = library_root(&store)?;
    Active::check_library_access(&app)?;
    let before: HashSet<String> = books.snapshot(&store, &root).into_iter().map(|b| b.fingerprint).collect();
    if store.read(|s| s.library_since.as_ref().is_none_or(|(at, _)| *at != root)) {
        store.update(|s| s.library_since = Some((root.clone(), crate::clock::now_ms()))).map_err(|e| e.to_string())?;
    }
    // Books whose format module is off stay listed, marked, and refuse to open.
    let disabled = store.read(modules::disabled_ids);
    // A file count while it runs, so the library screen shows the scan is moving.
    let entries = library::scan(
        &root,
        &library_cache(&store),
        &mut |done| {
            if done % 25 == 0 {
                let _ = app.emit("scan-progress", done);
            }
        },
    )
    .map_err(|e| format!("{}: {e}", root.display()))?;
    let views = store.read(|s| {
        entries
            .iter()
            .map(|e| {
                let reader = e.reader();
                LibraryBook {
                    key: e.path.clone(),
                    fingerprint: e.fingerprint.clone(),
                    title: e.book.title.clone(),
                    authors: e.book.authors.iter().map(metadata::Author::display).collect(),
                    author_sort: e.book.authors.first().map(metadata::Author::sort_name).unwrap_or_default(),
                    series: e.book.series.clone(),
                    series_index: e.book.series_index.clone(),
                    format: reader.map_or("", |r| r.label()),
                    disabled: reader.is_some_and(|r| disabled.contains(r.module())),
                    fraction: s.positions.get(&e.fingerprint).map(|p| p.fraction),
                    added: e.added,
                    has_cover: e.cover.is_some(),
                }
            })
            .collect()
    });
    let changed = entries.len() != before.len() || entries.iter().any(|e| !before.contains(&e.fingerprint));
    note_removals(&store, &before, &entries);
    books.replace(entries, &root);
    // Books came or went: the engine compares the library with the clouds. A
    // scan that found the same books (the phone looks every 30 s) asks nothing.
    if changed {
        engine.request_reconcile();
    }
    Ok(views)
}

/// Keeps Removed books in step with the folder: a book whose file is gone
/// since the last scan (deleted outside the app) counts as removed here; a
/// book back in the library no longer does.
fn note_removals(store: &Store, before: &HashSet<String>, entries: &[library::Entry]) {
    // A folder that suddenly holds nothing is far likelier unreachable (a
    // drive gone, a permission lost) than emptied on purpose.
    if entries.is_empty() {
        return;
    }
    let now: HashSet<&str> = entries.iter().map(|e| e.fingerprint.as_str()).collect();
    let gone: Vec<&String> = before.iter().filter(|f| !now.contains(f.as_str())).collect();
    let back = store.read(|s| s.removed_here.iter().any(|f| now.contains(f.as_str())));
    if gone.is_empty() && !back {
        return;
    }
    let _ = store.update(|s| {
        s.removed_here.retain(|f| !now.contains(f.as_str()));
        s.removed_here.extend(gone.into_iter().cloned());
    });
}

#[tauri::command(async)]
pub(crate) fn library_cover(store: State<Store>, books: State<LibraryBooks>, key: String) -> CmdResult<Response> {
    // Before the first scan the history's covers come from the cache, as recent_books found them.
    let cover = match books.find(&key).and_then(|entry| entry.cover) {
        Some(cover) => cover,
        None if books.is_scanned() => return Err("No cover".into()),
        None => {
            let root = library_root(&store)?;
            books.cached(&store, &root).into_iter().find(|b| b.path == key).and_then(|b| b.cover).ok_or("No cover")?
        }
    };
    std::fs::read(library_cache(&store).covers().join(cover)).map(Response::new).map_err(|e| e.to_string())
}

/// Whether a connected cloud holds the book: recorded, missing, or unknown
/// (the manifest could not be fetched).
#[derive(Serialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CloudState {
    Synced,
    Absent,
    Unknown,
}

/// One cloud's verdict for the details dialog.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudStatus {
    id: &'static str,
    label: String,
    state: CloudState,
}

/// Everything the details dialog shows about one library book, except the
/// cloud verdicts (those arrive separately, as they may wait on the network).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BookDetails {
    title: String,
    authors: Vec<String>,
    series: Option<String>,
    series_index: Option<String>,
    language: Option<String>,
    annotation: Option<String>,
    path: String,
    size: u64,
    format: String,
    fingerprint: String,
    catalog_source: Option<String>,
}

/// One connected cloud's verdict for the book: unknown when the manifest
/// cannot be fetched, otherwise recorded or not.
fn cloud_state(manifest: Result<Option<HashMap<String, crate::cloud::BookRecord>>, crate::sync_error::SyncError>, fingerprint: &str) -> CloudState {
    match manifest {
        Err(_) => CloudState::Unknown,
        Ok(Some(books)) if books.contains_key(fingerprint) => CloudState::Synced,
        Ok(_) => CloudState::Absent,
    }
}

/// The details dialog's contents for one library book, by fingerprint: the
/// reading window knows only that, so both windows share this command.
/// Nothing touches the network; the cloud verdicts come separately.
#[tauri::command(async)]
pub(crate) fn book_details(store: State<Store>, books: State<LibraryBooks>, fingerprint: String) -> CmdResult<BookDetails> {
    book_details_for(&store, &books, &fingerprint)
}

fn book_details_for(store: &Store, books: &LibraryBooks, fingerprint: &str) -> CmdResult<BookDetails> {
    let entry = books.find_by_fingerprint(store, fingerprint).ok_or("That book is no longer in the library")?;
    // The file's own blurb wins; a catalog summary fills the gap for books
    // whose files carry none.
    let annotation = entry.book.annotation.clone().or_else(|| store.read(|s| s.book_blurbs.get(&entry.fingerprint).cloned()));
    Ok(BookDetails {
        title: entry.book.title.clone(),
        authors: entry.book.authors.iter().map(|a| a.display()).collect(),
        series: entry.book.series.clone(),
        series_index: entry.book.series_index.clone(),
        language: entry.book.language.clone(),
        annotation,
        path: entry.path.clone(),
        size: entry.size,
        format: entry.reader().map(|r| r.label().to_string()).unwrap_or_default(),
        fingerprint: entry.fingerprint.clone(),
        catalog_source: store.read(|s| s.book_sources.get(&entry.fingerprint).cloned()),
    })
}

/// The cloud verdicts for one fingerprint, for the details dialog: one per
/// cloud that syncs (its module and progress sync on, a login kept). Slow when
/// the login is stale (the manifest fetch has the network timeout); the
/// dialog shows local details first and fills these in on arrival.
#[tauri::command(async)]
pub(crate) fn book_cloud(store: State<Store>, fingerprint: String) -> CmdResult<Vec<CloudStatus>> {
    book_cloud_for(&store, &fingerprint)
}

/// The cloud verdicts for one fingerprint (see `book_cloud`).
fn book_cloud_for(store: &Store, fingerprint: &str) -> CmdResult<Vec<CloudStatus>> {
    let labels: HashMap<&str, &str> = store.read(modules::list).into_iter().map(|m| (m.id, m.label)).collect();
    let mut out = Vec::new();
    for cloud in crate::cloud::stores() {
        let module = cloud.module();
        let off = store.read(|s| modules::is_off(s, module) || modules::is_off(s, modules::id::PROGRESS_SYNC));
        if off || !cloud.connected() {
            continue;
        }
        out.push(CloudStatus {
            id: module,
            label: labels.get(module).copied().unwrap_or(module).into(),
            state: cloud_state(crate::cloud::manifest(cloud), fingerprint),
        });
    }
    Ok(out)
}

/// Remembers which catalog a book came from and what it said about it, for
/// the details dialog. Best-effort: a good download must not fail over
/// bookkeeping.
pub(crate) fn record_book_source(store: &Store, fingerprint: &str, source: &str, blurb: Option<&str>) -> CmdResult<()> {
    store
        .update(|s| {
            s.book_sources.insert(fingerprint.into(), source.into());
            if let Some(text) = blurb.map(str::trim).filter(|t| !t.is_empty()) {
                s.book_blurbs.insert(fingerprint.into(), text.into());
            }
        })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Author, Book};
    use std::collections::HashMap;

    fn bookish_state() -> (tempfile::TempDir, Store, LibraryBooks) {
        bookish_state_with(Some("Blurb."))
    }

    fn bookish_state_with(annotation: Option<&str>) -> (tempfile::TempDir, Store, LibraryBooks) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(dir.path().join("breader.json"));
        store
            .update(|s| {
                s.settings.insert("disabledModules".into(), serde_json::json!([crate::modules::id::DROPBOX, crate::modules::id::GOOGLE_DRIVE]));
            })
            .unwrap();
        let books = LibraryBooks::default();
        books.replace(
            vec![crate::library::Entry {
                path: "Author/Book.epub".into(),
                size: 10,
                modified: 0,
                fingerprint: "fp1".into(),
                book: Book {
                    title: "T".into(),
                    authors: vec![Author { first: "A".into(), last: "B".into() }],
                    series: Some("S".into()),
                    series_index: Some("1".into()),
                    language: Some("en".into()),
                    annotation: annotation.map(str::to_string),
                },
                cover: Some("fp1.jpg".into()),
                added: 0,
            }],
            dir.path(),
        );
        (dir, store, books)
    }

    #[test]
    fn cloud_state_names_every_case() {
        let books: HashMap<String, crate::cloud::BookRecord> =
            [("fp".into(), crate::cloud::BookRecord { name: "a".into(), size: 1, sha256: "s".into(), added: 0, removed: None })]
                .into_iter()
                .collect();
        assert_eq!(cloud_state(Err("down".into()), "fp"), CloudState::Unknown);
        assert_eq!(cloud_state(Ok(None), "fp"), CloudState::Absent);
        assert_eq!(cloud_state(Ok(Some(books.clone())), "fp"), CloudState::Synced);
        assert_eq!(cloud_state(Ok(Some(books)), "other"), CloudState::Absent);
    }

    #[test]
    fn book_details_names_everything() {
        let (_dir, store, books) = bookish_state();
        record_book_source(&store, "fp1", "CWA", None).unwrap();
        let d = book_details_for(&store, &books, "fp1").unwrap();
        assert_eq!(d.title, "T");
        assert_eq!(d.authors, vec!["A B".to_string()]);
        assert_eq!(d.series.as_deref(), Some("S"));
        assert_eq!(d.series_index.as_deref(), Some("1"));
        assert_eq!(d.language.as_deref(), Some("en"));
        assert_eq!(d.annotation.as_deref(), Some("Blurb."));
        assert_eq!(d.path, "Author/Book.epub");
        assert_eq!(d.size, 10);
        assert_eq!(d.format, "EPUB");
        assert_eq!(d.fingerprint, "fp1");
        assert_eq!(d.catalog_source.as_deref(), Some("CWA"));
    }

    #[test]
    fn record_book_source_remembers_the_catalog() {
        let (_dir, store, _books) = bookish_state();
        record_book_source(&store, "fp9", "CWA", Some("A blurb.  ")).unwrap();
        assert_eq!(store.read(|s| s.book_sources.get("fp9").cloned()).as_deref(), Some("CWA"));
        assert_eq!(store.read(|s| s.book_blurbs.get("fp9").cloned()).as_deref(), Some("A blurb."));
        record_book_source(&store, "fp8", "CWA", Some("   ")).unwrap();
        assert_eq!(store.read(|s| s.book_blurbs.get("fp8").cloned()), None);
    }

    #[test]
    fn book_details_falls_back_to_the_catalog_blurb() {
        let (_dir, store, books) = bookish_state_with(None);
        store.update(|s| { s.book_blurbs.insert("fp1".into(), "Catalog says hi.".into()); }).unwrap();
        let d = book_details_for(&store, &books, "fp1").unwrap();
        assert_eq!(d.annotation.as_deref(), Some("Catalog says hi."));
    }

    #[test]
    fn book_details_refuses_a_missing_book() {
        let (_dir, store, books) = bookish_state();
        assert!(book_details_for(&store, &books, "Missing/Book.epub").is_err());
    }

    #[test]
    fn book_cloud_lists_only_clouds_that_sync() {
        // The test state switches the clouds off: no rows, no network.
        let (_dir, store, _books) = bookish_state();
        assert!(book_cloud_for(&store, "fp1").unwrap().is_empty());
    }
}

/// Opens a library book: in a window of its own, or on a phone in its one window.
#[tauri::command(async)]
pub(crate) fn open_library_book(
    window: Window,
    store: State<Store>,
    books: State<LibraryBooks>,
    key: String,
    from_start: bool,
) -> CmdResult<()> {
    let path = books.path_of(&store, &key)?;
    if let Some(reader) = formats::of(&path) {
        store.read(|s| formats::check(s, reader))?;
    }
    show_book(&window, path, from_start);
    Ok(())
}

/// Deletes a book from the library: the file goes to the Recycle Bin (a phone
/// deletes it). Its reading position and bookmarks stay, by its content, so
/// the book picks up where it was if it comes back; the history forgets it.
#[tauri::command(async)]
pub(crate) fn delete_library_book(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, key: String) -> CmdResult<()> {
    let entry = books.entry(&key)?;
    let path = entry.absolute(&library_root(&store)?);
    // The clouds keep it until it is removed from them too (Removed books);
    // noted first, so the engine never downloads it back in between.
    store.update(|s| { s.removed_here.insert(entry.fingerprint.clone()); }).map_err(|e| e.to_string())?;
    if let Err(e) = Active::delete_book(&path) {
        let _ = store.update(|s| { s.removed_here.remove(&entry.fingerprint); });
        return Err(format!("{}: {e}", paths::file_name(&path)));
    }
    books.forget(&key);
    store.update(|s| s.recent.retain(|r| r.fingerprint != entry.fingerprint)).map_err(|e| e.to_string())?;
    let _ = app.emit("library-updated", ());
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Added {
    /// Where the book is now, inside the library.
    pub(crate) path: String,
    /// The library already held this book, so nothing was copied or moved.
    existed: bool,
    /// Moved, but the original (a phone's document) couldn't be deleted.
    original_kept: bool,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AddMode {
    Move,
    Copy,
}

/// Copies or moves the window's book into the library under the naming
/// template; the window then shows the library copy. If the library already
/// holds the same book (under any name), nothing is copied or moved and the
/// window switches to that copy.
#[tauri::command(async)]
pub(crate) fn add_to_library(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    books: State<LibraryBooks>,
    originals: State<Originals>,
    mode: AddMode,
) -> CmdResult<Added> {
    let source = saving_window(&windows, &window)?;
    // A phone's book from outside is the app's own copy: it moves in, and
    // Move then deletes the document it came from.
    let original = originals.0.lock().unwrap().remove(&source);
    let placed = put_in_library(&store, &books, &source, if original.is_some() { AddMode::Move } else { mode })?;
    let mut added = placed.added;
    if let (Some(original), AddMode::Move, false) = (original, mode, added.existed) {
        added.original_kept = !Active::delete_original(&original);
    }
    let target = library::join(&library_root(&store)?, &added.path);
    windows.set_path(window.label(), target.clone());
    repath_recent(&store, &[(placed.fingerprint, target)])?;
    let _ = window.emit("library-updated", ());
    Ok(added)
}

/// A book moved or copied into the library, with its fingerprint.
pub(crate) struct Placed {
    pub(crate) added: Added,
    pub(crate) fingerprint: String,
}

/// Moves or copies the book file `source` into the library under the naming
/// template. If the library already holds the same book (under any name),
/// nothing is moved or copied.
pub(crate) fn put_in_library(store: &Store, books: &LibraryBooks, source: &Path, mode: AddMode) -> CmdResult<Placed> {
    let root = library_root(store)?;
    let err = |e: std::io::Error| format!("{}: {e}", paths::file_name(source));
    let reader = formats::of(source).ok_or("Not a book file")?;
    store.read(|s| formats::check(s, reader))?;
    let (fingerprint, meta) = library::read_book(source).map_err(err)?;
    let known = books.duplicate(store, &root, &fingerprint).map(naming::Placement::Exists);
    let placement = match known {
        Some(found) => found,
        None => {
            let wanted = naming::template_path(&meta.book, reader.extension_for(&paths::file_name(source)).unwrap_or("epub"));
            naming::place(&root, &wanted, &fingerprint, &Default::default()).map_err(err)?
        }
    };
    let added = match placement {
        naming::Placement::Free(rel) => {
            let target = library::join(&root, &rel);
            match mode {
                AddMode::Move => naming::move_file(source, &target),
                AddMode::Copy => naming::copy_file(source, &target),
            }
            .map_err(err)?;
            Added { path: rel, ..Default::default() }
        }
        naming::Placement::Exists(rel) => Added { path: rel, existed: true, ..Default::default() },
    };
    Ok(Placed { added, fingerprint })
}

/// Points recent-list entries at the books' new places (by fingerprint), in one write.
fn repath_recent(store: &Store, moves: &[(String, PathBuf)]) -> CmdResult<()> {
    store
        .update(|s| {
            for (fingerprint, path) in moves {
                s.recent.iter_mut().filter(|r| &r.fingerprint == fingerprint).for_each(|r| r.path = path.clone());
            }
        })
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub(crate) struct Rename {
    from: String,
    to: String,
}

/// The renames that would make the last scan's books follow the naming template.
#[tauri::command(async)]
pub(crate) fn organize_preview(store: State<Store>, books: State<LibraryBooks>) -> CmdResult<Vec<Rename>> {
    store.read(|s| modules::require(s, crate::modules::id::ORGANIZE))?;
    let root = library_root(&store)?;
    let plan = library::organize_plan(&root, &books.snapshot(&store, &root)).map_err(|e| e.to_string())?;
    Ok(plan.into_iter().map(|(entry, to)| Rename { from: entry.path, to }).collect())
}

/// Applies the plan (recomputed here, so the frontend can't name arbitrary
/// files). Returns how many books were renamed.
#[tauri::command(async)]
pub(crate) fn organize_apply(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, windows: State<ReaderWindows>) -> CmdResult<usize> {
    store.read(|s| modules::require(s, crate::modules::id::ORGANIZE))?;
    let root = library_root(&store)?;
    let plan = library::organize_plan(&root, &books.snapshot(&store, &root)).map_err(|e| e.to_string())?;
    let mut moved = Vec::new();
    let result = plan.iter().try_for_each(|(entry, to)| {
        let from = entry.absolute(&root);
        let target = library::join(&root, to);
        naming::move_file(&from, &target).map_err(|e| format!("{}: {e}", entry.path))?;
        if let Some(dir) = from.parent() {
            naming::remove_empty_dirs(&root, dir.to_path_buf());
        }
        windows.repath(&from, &target);
        moved.push((entry.fingerprint.clone(), target));
        Ok::<_, String>(())
    });
    // Books moved before a failure are recorded too.
    repath_recent(&store, &moved)?;
    let _ = app.emit("library-updated", ());
    result.map(|()| moved.len())
}
