// Android uses only `Launch`: it has no command line.
#[cfg_attr(mobile, allow(dead_code))]
mod catalog;
mod cli;
mod clock;
#[cfg(desktop)]
#[cfg_attr(not(windows), path = "console_stdout.rs")]
mod console;
// Android keeps its data in the app's storage: only `resolve_state_file` is shared.
#[cfg_attr(mobile, allow(dead_code))]
mod data_dir;
mod fingerprint;
mod kosync;
mod library;
mod metadata;
mod naming;
mod net;
mod opds;
mod paths;
#[cfg_attr(not(windows), path = "register_unsupported.rs")]
mod register;
// Where the library lives: a folder of the reader's choice, or Documents/Books on Android.
#[cfg_attr(desktop, path = "platform_desktop.rs")]
#[cfg_attr(mobile, path = "platform_android.rs")]
mod platform;
mod store;
mod updates;
#[cfg(desktop)]
mod version;
mod xml;

#[cfg(desktop)]
use cli::Command;
use cli::Launch;
use clock::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
#[cfg(desktop)]
use store::WindowGeometry;
use store::{Position, RecentBook, Store};
use tauri::ipc::Response;
#[cfg(desktop)]
use tauri::DragDropEvent;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent};
use tauri_plugin_opener::OpenerExt;

#[cfg(desktop)]
const DEFAULT_WIDTH: f64 = 1100.0;
#[cfg(desktop)]
const DEFAULT_HEIGHT: f64 = 800.0;

/// What each reader window shows (its book and `--no-save`), by window label.
/// The frontend never passes file paths: it can only read the book the
/// backend assigned to its window, so book content cannot make the app read
/// arbitrary files.
#[derive(Default)]
struct ReaderWindows(Mutex<HashMap<String, Launch>>);

impl ReaderWindows {
    fn get(&self, label: &str) -> Launch {
        self.0.lock().unwrap().get(label).cloned().unwrap_or_default()
    }

    fn path_of(&self, label: &str) -> Result<PathBuf, String> {
        self.get(label).file.ok_or_else(|| "No book is open".into())
    }

    fn insert(&self, label: &str, launch: Launch) {
        self.0.lock().unwrap().insert(label.to_string(), launch);
    }

    fn remove(&self, label: &str) {
        self.0.lock().unwrap().remove(label);
    }

    fn set_path(&self, label: &str, path: PathBuf) {
        self.0.lock().unwrap().entry(label.to_string()).or_default().file = Some(path);
    }

    /// Takes the window's "from the beginning" request: it holds for one load.
    fn take_from_start(&self, label: &str) -> bool {
        self.0.lock().unwrap().get_mut(label).is_some_and(|w| std::mem::take(&mut w.from_start))
    }

    /// The window, other than `except`, already showing `path`.
    fn showing(&self, path: &Path, except: Option<&str>) -> Option<String> {
        let windows = self.0.lock().unwrap();
        windows
            .iter()
            .filter(|(label, _)| Some(label.as_str()) != except)
            .find(|(_, w)| w.file.as_deref().is_some_and(|file| paths::same_file(file, path)))
            .map(|(label, _)| label.clone())
    }

    /// The library window's label: the window without a book.
    fn library_window(&self) -> Option<String> {
        let windows = self.0.lock().unwrap();
        windows.iter().find(|(_, w)| w.file.is_none()).map(|(label, _)| label.clone())
    }

    /// Points windows showing a file that moved at its new place.
    fn repath(&self, from: &Path, to: &Path) {
        for w in self.0.lock().unwrap().values_mut() {
            if w.file.as_deref().is_some_and(|file| paths::same_file(file, from)) {
                w.file = Some(to.to_path_buf());
            }
        }
    }
}

/// Brings a window forward; false when it is gone.
fn focus_window(app: &AppHandle, label: Option<String>) -> bool {
    let Some(window) = label.and_then(|label| app.get_webview_window(&label)) else { return false };
    #[cfg(desktop)]
    let _ = window.unminimize();
    window.set_focus().is_ok()
}

/// Brings forward the window (other than `except`) already showing `path`;
/// false when there is none.
fn focus_showing(app: &AppHandle, path: &Path, except: Option<&str>) -> bool {
    focus_window(app, app.state::<ReaderWindows>().showing(path, except))
}

/// Shows `path` in `window` and tells its frontend to load it (`from_start`:
/// at the beginning, not the saved position), unless another window already
/// shows that book; then that window comes forward instead (and, from the
/// start, reads it again). The library window stays the library: the book gets
/// a window of its own. A phone has one window, so there the book opens in it.
fn show_book(window: &Window, path: PathBuf, from_start: bool) {
    if cfg!(mobile) {
        return show_here(window, Some(path), from_start);
    }
    let windows = window.state::<ReaderWindows>();
    let current = windows.get(window.label());
    if let Some(label) = windows.showing(&path, current.file.is_some().then(|| window.label())) {
        if from_start {
            let no_save = windows.get(&label).no_save;
            windows.insert(&label, Launch { file: Some(path), no_save, from_start });
            let _ = window.emit_to(label.as_str(), "book-changed", ());
        }
        focus_window(window.app_handle(), Some(label));
        return;
    }
    // A `--no-save` library window passes that on to the books it opens.
    let launch = Launch { file: Some(path), no_save: current.no_save, from_start };
    if current.file.is_none() {
        let _ = open_window(window.app_handle(), launch);
        return;
    }
    windows.insert(window.label(), launch);
    let _ = window.emit_to(window.label(), "book-changed", ());
}

/// Shows a book, or the library for `None`, in `window` itself. A phone has
/// one window, so everything opens there and Back returns to the library.
fn show_here(window: &Window, file: Option<PathBuf>, from_start: bool) {
    let windows = window.state::<ReaderWindows>();
    let no_save = windows.get(window.label()).no_save;
    windows.insert(window.label(), Launch { file, no_save, from_start });
    let _ = window.emit_to(window.label(), "book-changed", ());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BookInfo {
    file_name: String,
    fingerprint: String,
    position: Option<Position>,
    no_save: bool,
    offer: Option<Offer>,
}

/// What the bar under a book offers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum Offer {
    /// The book lies outside the library: copy or move it in.
    Add,
    /// No library folder has been chosen yet (the app was first started on a book).
    Setup,
}

#[tauri::command(async)]
fn current_book(window: Window, windows: State<ReaderWindows>, store: State<Store>) -> Result<Option<BookInfo>, String> {
    let Launch { file: Some(path), no_save, .. } = windows.get(window.label()) else {
        return Ok(None);
    };
    // "From the beginning" in the library: this time without the saved position.
    let from_start = windows.take_from_start(window.label());
    let file_name = paths::file_name(&path);
    let fingerprint = fingerprint::fingerprint(&path).map_err(|e| format!("{file_name}: {e}"))?;
    let (position, offer) = store.read(|s| {
        let offer = match s.library.as_deref() {
            _ if no_save => None,
            None => Some(Offer::Setup),
            Some(root) if !paths::is_inside(root, &path) && !s.declined.contains(&fingerprint) => Some(Offer::Add),
            Some(_) => None,
        };
        (s.positions.get(&fingerprint).cloned().filter(|_| !from_start), offer)
    });
    Ok(Some(BookInfo { file_name, fingerprint, position, no_save, offer }))
}

#[tauri::command(async)]
fn read_book(window: Window, windows: State<ReaderWindows>) -> Result<Response, String> {
    let path = windows.path_of(window.label())?;
    std::fs::read(path).map(Response::new).map_err(|e| e.to_string())
}

/// Takes the fingerprint from the frontend so a position that is still
/// pending when another book replaces it is saved under the right book.
#[tauri::command(async)]
fn save_position(store: State<Store>, fingerprint: String, mut position: Position) -> Result<(), String> {
    store
        .update(|s| {
            // The time says when the reader moved there (sync compares it), so
            // saving the same place again (a reflow) keeps it.
            position.time = match s.positions.get(&fingerprint) {
                Some(old) if old.cfi == position.cfi => old.time,
                _ => now_ms(),
            };
            s.positions.insert(fingerprint, position);
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(store: State<Store>) -> Map<String, Value> {
    store.read(|s| s.settings.clone())
}

/// Merges changed settings (so windows never overwrite each other's changes)
/// and passes them on to the other windows.
#[tauri::command(async)]
fn update_settings(window: Window, store: State<Store>, changes: Map<String, Value>) -> Result<(), String> {
    store.update(|s| s.settings.extend(changes.clone())).map_err(|e| e.to_string())?;
    for label in window.app_handle().webview_windows().into_keys().filter(|l| l != window.label()) {
        let _ = window.emit_to(&label, "settings-changed", &changes);
    }
    Ok(())
}

/// Adds the window's book to the recent list, with the title and author the
/// frontend read from it.
#[tauri::command(async)]
fn remember_book(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    fingerprint: String,
    title: String,
    author: String,
) -> Result<(), String> {
    let Launch { file: Some(path), no_save: false, .. } = windows.get(window.label()) else {
        return Ok(());
    };
    let book = RecentBook { path, title, author, fingerprint };
    // Reopening the latest book (or reloading it) changes nothing: skip the write.
    if store.read(|s| s.recent.first() == Some(&book)) {
        return Ok(());
    }
    store.update(|s| s.remember(book)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentView {
    /// Identifies the entry (the book's fingerprint), so a click still finds
    /// the right book after the list changed in another window.
    key: String,
    title: String,
    author: String,
    file_name: String,
    /// Share read, if a position is saved.
    fraction: Option<f64>,
    /// False when the file has been moved or deleted.
    exists: bool,
    /// The library's key for the same book (by content), when it has a cover there.
    cover_key: Option<String>,
}

#[tauri::command(async)]
fn recent_books(store: State<Store>, library: State<LibraryBooks>) -> Vec<RecentView> {
    let books = store.read(|s| {
        let fraction = |r: &RecentBook| s.positions.get(&r.fingerprint).map(|p| p.fraction);
        s.recent.iter().map(|r| (r.clone(), fraction(r))).collect::<Vec<_>>()
    });
    // The library's covers, by the books' content (a history entry may lie anywhere).
    let covers: HashMap<String, String> = library_root(&store)
        .map(|root| library.snapshot(&store, &root))
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.cover.is_some())
        .map(|e| (e.fingerprint, e.path))
        .collect();
    // Checked outside the store lock: a slow drive shouldn't hold up other windows.
    books
        .into_iter()
        .map(|(r, fraction)| RecentView {
            file_name: paths::file_name(&r.path),
            exists: r.path.is_file(),
            cover_key: covers.get(&r.fingerprint).cloned(),
            key: r.fingerprint,
            title: r.title,
            author: r.author,
            fraction,
        })
        .collect()
}

/// Opens a recent book in this window. Books are picked by their key, so the
/// frontend still never supplies a path.
#[tauri::command(async)]
fn open_recent(window: Window, store: State<Store>, key: String) -> Result<(), String> {
    let path = store
        .read(|s| s.recent.iter().find(|r| r.fingerprint == key).map(|r| r.path.clone()))
        .ok_or("That book is no longer in the recent list")?;
    show_book(&window, path, false);
    Ok(())
}

#[tauri::command(async)]
fn forget_recent(store: State<Store>, key: String) -> Result<(), String> {
    store.update(|s| s.recent.retain(|r| r.fingerprint != key)).map_err(|e| e.to_string())
}

/// Forgets the recent books list. Reading positions are kept, so reopened books resume.
#[tauri::command(async)]
fn clear_recent(store: State<Store>) -> Result<(), String> {
    store.update(|s| s.recent.clear()).map_err(|e| e.to_string())
}

/// Registers the app for its book file types (see register::register).
#[tauri::command(async)]
fn register_file_types() -> Result<(), String> {
    std::env::current_exe().and_then(|exe| register::register(&exe)).map_err(|e| e.to_string())
}

/// Removes the app's file associations again.
#[tauri::command(async)]
fn unregister_file_types() -> Result<(), String> {
    register::unregister().map_err(|e| e.to_string())
}

/// Whether the app is registered for its book file types.
#[tauri::command(async)]
fn file_types_registered() -> bool {
    register::registered()
}

/// The app version (from Cargo.toml), shown in the About panel.
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Opens the app's repository on GitHub in the browser (Settings → About).
#[tauri::command]
fn open_repository(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(updates::REPOSITORY, None::<&str>).map_err(|e| e.to_string())
}

/// Opens the latest release's page, where its files are, in the browser.
#[tauri::command]
fn open_latest_release(app: AppHandle) -> Result<(), String> {
    app.opener().open_url(format!("{}/releases/latest", updates::REPOSITORY), None::<&str>).map_err(|e| e.to_string())
}

/// Asks GitHub for the latest release and compares it with this copy.
#[tauri::command(async)]
fn check_for_update() -> Result<updates::UpdateCheck, String> {
    updates::check()
}

/// Ctrl+O (on a phone, "Add book…"): asks for a book and opens it in this window.
#[tauri::command(async)]
fn pick_book(window: Window, originals: State<Originals>) -> Result<(), String> {
    if let Some(book) = platform::pick_book(&window)? {
        open_outside(&window, &originals, book);
    }
    Ok(())
}

/// Opens a book handed to the app ("Open with", "Share" on a phone), if one waits.
#[tauri::command(async)]
fn open_incoming(window: Window, originals: State<Originals>) -> Result<(), String> {
    if let Some(book) = platform::take_incoming()? {
        open_outside(&window, &originals, book);
    }
    Ok(())
}

/// A book from outside the library, as the platform hands it over: on the
/// desktop the file itself, on a phone a copy in the app's storage and the
/// document it came from (deleted when the reader moves the book in).
pub struct Outside {
    pub path: PathBuf,
    pub original: Option<String>,
}

/// The documents that books from outside were copied from on a phone (the
/// copy → its `content://` original), for Move.
#[derive(Default)]
struct Originals(Mutex<HashMap<PathBuf, String>>);

fn open_outside(window: &Window, originals: &Originals, book: Outside) {
    if let Some(original) = book.original {
        originals.0.lock().unwrap().insert(book.path.clone(), original);
    }
    show_book(window, book.path, false);
}

/// A book is open (or not): on a phone the screen stays on, the status bar
/// hides, and the volume keys turn pages if that setting is on.
#[tauri::command(async)]
fn set_reading(on: bool, volume_keys: bool) -> Result<(), String> {
    platform::set_reading(on, volume_keys)
}

/// Opens a window for `launch`, or brings forward the one already showing its
/// file (or, without a file, the library window).
///
/// Every command that can get here must be `async`: a sync command runs on the
/// main thread, and building a window there deadlocks on Windows.
fn open_or_focus(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    let focused = match launch.file.as_deref() {
        Some(file) => focus_showing(app, file, None),
        None => focus_window(app, app.state::<ReaderWindows>().library_window()),
    };
    if focused {
        return Ok(());
    }
    open_window(app, launch)
}

// ---- Library ------------------------------------------------------------------

/// The books found by the last scan. The frontend names books by their key
/// (the path inside the library), which is only accepted when the scan found
/// it, so the frontend still cannot make the app open arbitrary files.
#[derive(Default)]
struct LibraryBooks(Mutex<Vec<library::Entry>>);

impl LibraryBooks {
    fn entry(&self, key: &str) -> Result<library::Entry, String> {
        let books = self.0.lock().unwrap();
        books.iter().find(|b| b.path == key).cloned().ok_or_else(|| "That book is no longer in the library".into())
    }

    fn path_of(&self, store: &Store, key: &str) -> Result<PathBuf, String> {
        Ok(self.entry(key)?.absolute(&library_root(store)?))
    }

    /// Takes a deleted book out of the list.
    fn forget(&self, key: &str) {
        self.0.lock().unwrap().retain(|b| b.path != key);
    }

    /// The last scan's books; before any scan in this run, the cached ones.
    fn snapshot(&self, store: &Store, root: &Path) -> Vec<library::Entry> {
        let books = self.0.lock().unwrap();
        if books.is_empty() { library_cache(store).load(root) } else { books.clone() }
    }
}

fn library_root(store: &Store) -> Result<PathBuf, String> {
    store.read(|s| s.library.clone()).ok_or_else(|| "No library folder is set".into())
}

/// The app's cache folder, next to its settings.
const CACHE_DIR: &str = "breader-cache";

fn library_cache(store: &Store) -> library::Cache {
    library::Cache::new(store.dir().join(CACHE_DIR))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    /// The library folder, once chosen.
    folder: Option<String>,
    /// What the first-run screen offers.
    suggested: String,
}

#[tauri::command(async)]
fn library_info(app: AppHandle, store: State<Store>) -> Result<LibraryInfo, String> {
    Ok(LibraryInfo {
        folder: store.read(|s| s.library.as_ref().map(|p| p.display().to_string())),
        suggested: platform::suggested_library(&app)?.display().to_string(),
    })
}

/// Sets the library folder: the suggested one, or one the reader picks.
/// Returns the folder, or `None` when the picker was cancelled.
#[tauri::command(async)]
fn set_library(window: Window, store: State<Store>, books: State<LibraryBooks>, pick: bool) -> Result<Option<String>, String> {
    let Some(folder) = platform::chosen_library(&window, pick)? else { return Ok(None) };
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let folder = std::path::absolute(&folder).unwrap_or(folder);
    store.update(|s| s.library = Some(folder.clone())).map_err(|e| e.to_string())?;
    // The last scan's keys belong to the old folder.
    books.0.lock().unwrap().clear();
    let _ = window.emit("library-changed", ());
    Ok(Some(folder.display().to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryBook {
    key: String,
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
}

/// Scans the library folder (cheap after the first time: see library::scan).
#[tauri::command(async)]
fn scan_library(app: AppHandle, store: State<Store>, books: State<LibraryBooks>) -> Result<Vec<LibraryBook>, String> {
    let root = library_root(&store)?;
    platform::check_library_access(&app)?;
    let entries = library::scan(&root, &library_cache(&store)).map_err(|e| format!("{}: {e}", root.display()))?;
    let views = store.read(|s| {
        entries
            .iter()
            .map(|e| LibraryBook {
                key: e.path.clone(),
                title: e.book.title.clone(),
                authors: e.book.authors.iter().map(metadata::Author::display).collect(),
                author_sort: e.book.authors.first().map(metadata::Author::sort_name).unwrap_or_default(),
                series: e.book.series.clone(),
                series_index: e.book.series_index.clone(),
                format: e.format().map_or("", metadata::Format::label),
                fraction: s.positions.get(&e.fingerprint).map(|p| p.fraction),
                added: e.added,
                has_cover: e.cover.is_some(),
            })
            .collect()
    });
    *books.0.lock().unwrap() = entries;
    Ok(views)
}

#[tauri::command(async)]
fn library_cover(store: State<Store>, books: State<LibraryBooks>, key: String) -> Result<Response, String> {
    // Before the first scan the history's covers come from the cache, as recent_books found them.
    let cover = match books.entry(&key) {
        Ok(entry) => entry.cover,
        Err(e) => library_root(&store)
            .ok()
            .and_then(|root| books.snapshot(&store, &root).into_iter().find(|b| b.path == key))
            .ok_or(e)?
            .cover,
    }
    .ok_or("No cover")?;
    std::fs::read(library_cache(&store).covers().join(cover)).map(Response::new).map_err(|e| e.to_string())
}

/// Opens a library book: in a window of its own, or on a phone in its one window.
#[tauri::command(async)]
fn open_library_book(
    window: Window,
    store: State<Store>,
    books: State<LibraryBooks>,
    key: String,
    from_start: bool,
) -> Result<(), String> {
    show_book(&window, books.path_of(&store, &key)?, from_start);
    Ok(())
}

/// Deletes a book from the library: the file goes to the Recycle Bin (a phone
/// deletes it). Its reading position and bookmarks stay, by its content, so
/// the book picks up where it was if it comes back; the history forgets it.
#[tauri::command(async)]
fn delete_library_book(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, key: String) -> Result<(), String> {
    let entry = books.entry(&key)?;
    let path = entry.absolute(&library_root(&store)?);
    platform::delete_book(&path).map_err(|e| format!("{}: {e}", paths::file_name(&path)))?;
    books.forget(&key);
    if store.read(|s| s.recent.iter().any(|r| r.fingerprint == entry.fingerprint)) {
        store.update(|s| s.recent.retain(|r| r.fingerprint != entry.fingerprint)).map_err(|e| e.to_string())?;
    }
    let _ = app.emit("library-updated", ());
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct Added {
    /// Where the book is now, inside the library.
    path: String,
    /// The library already held this book, so nothing was copied or moved.
    existed: bool,
    /// Moved, but the original (a phone's document) couldn't be deleted.
    original_kept: bool,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
enum AddMode {
    Move,
    Copy,
}

/// Fails for a `--no-save` window, which keeps nothing.
fn saving_window(windows: &ReaderWindows, window: &Window) -> Result<PathBuf, String> {
    let launch = windows.get(window.label());
    if launch.no_save {
        return Err("This window keeps nothing (--no-save)".into());
    }
    launch.file.ok_or_else(|| "No book is open".into())
}

/// Copies or moves the window's book into the library under the naming
/// template; the window then shows the library copy. If the library already
/// holds the same book (under any name), nothing is copied or moved and the
/// window switches to that copy.
#[tauri::command(async)]
fn add_to_library(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    books: State<LibraryBooks>,
    originals: State<Originals>,
    mode: AddMode,
) -> Result<Added, String> {
    let source = saving_window(&windows, &window)?;
    // A phone's book from outside is the app's own copy: it moves in, and
    // Move then deletes the document it came from.
    let original = originals.0.lock().unwrap().remove(&source);
    let (mut added, fingerprint) = put_in_library(&store, &books, &source, if original.is_some() { AddMode::Move } else { mode })?;
    if let (Some(original), AddMode::Move, false) = (original, mode, added.existed) {
        added.original_kept = !platform::delete_original(&original);
    }
    let target = library::join(&library_root(&store)?, &added.path);
    windows.set_path(window.label(), target.clone());
    repath_recent(&store, &[(fingerprint, target)])?;
    let _ = window.emit("library-updated", ());
    Ok(added)
}

/// Moves or copies the book file `source` into the library under the naming
/// template. If the library already holds the same book (under any name),
/// nothing is moved or copied. Also returns the book's fingerprint.
fn put_in_library(store: &Store, books: &LibraryBooks, source: &Path, mode: AddMode) -> Result<(Added, String), String> {
    let root = library_root(store)?;
    let err = |e: std::io::Error| format!("{}: {e}", paths::file_name(source));
    let format = metadata::Format::of(source).ok_or("Not a book file")?;
    let (fingerprint, meta) = library::read_book(source).map_err(err)?;
    let known = books
        .snapshot(store, &root)
        .into_iter()
        .find(|e| e.fingerprint == fingerprint && e.absolute(&root).is_file())
        .map(|e| naming::Placement::Exists(e.path));
    let placement = match known {
        Some(found) => found,
        None => {
            let wanted = naming::template_path(&meta.book, format);
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
    Ok((added, fingerprint))
}

// ---- Progress sync (KOSync) -----------------------------------------------------

/// The source that syncs progress, if one is set.
fn sync_source(store: &Store) -> Option<catalog::Source> {
    store.read(|s| s.sources.iter().find(|source| source.sync).cloned())
}

/// This install's id for the sync server, made once.
fn device_id(store: &Store) -> Result<String, String> {
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
/// (there is a sync source, and the window keeps positions).
#[tauri::command(async)]
fn sync_document(window: Window, store: State<Store>) -> Result<Option<String>, String> {
    let launch = window.state::<ReaderWindows>().get(window.label());
    let (Some(path), false, Some(_)) = (launch.file, launch.no_save, sync_source(&store)) else { return Ok(None) };
    kosync::document_id(&path).map(Some).map_err(|e| e.to_string())
}

/// A newer position from another device, offered when a book opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncOffer {
    cfi: String,
    fraction: f64,
    device: String,
    /// When the other device was there (ms since the Unix epoch).
    time: u64,
}

/// What the sync server's position means for the book just opened.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct SyncState {
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
fn sync_pull(store: State<Store>, document: String, fingerprint: String) -> Result<SyncState, String> {
    let Some(source) = sync_source(&store) else { return Ok(SyncState::default()) };
    let remote = kosync::pull(&source, &document)?;
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

/// Sends a book's position to the sync server.
#[tauri::command(async)]
fn sync_push(store: State<Store>, document: String, cfi: String, fraction: f64) -> Result<(), String> {
    let Some(source) = sync_source(&store) else { return Ok(()) };
    let device_id = device_id(&store)?;
    kosync::push(&source, &kosync::Update { document: &document, progress: &cfi, percentage: fraction, device: DEVICE_NAME, device_id: &device_id })
}

// ---- Catalogs -----------------------------------------------------------------

/// Search URL templates by source id, read once from each catalog.
#[derive(Default)]
struct SearchTemplates(Mutex<HashMap<String, String>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceView {
    #[serde(flatten)]
    source: catalog::Source,
    has_password: bool,
}

#[tauri::command(async)]
fn catalog_sources(store: State<Store>) -> Vec<SourceView> {
    let sources = store.read(|s| s.sources.clone());
    sources.into_iter().map(|source| SourceView { has_password: platform::has_password(&source.id), source }).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceInput {
    /// Missing for a new source.
    id: Option<String>,
    name: String,
    url: String,
    user: String,
    /// A new password; missing or empty keeps the one kept already.
    password: Option<String>,
    sync: bool,
}

#[derive(Serialize)]
struct SavedSource {
    id: String,
    /// Said when the address is plain HTTP across the internet.
    warning: Option<String>,
}

/// Adds or changes a catalog source, after finding its OPDS catalog (which
/// also checks the login). One source at most syncs progress.
#[tauri::command(async)]
fn save_catalog_source(
    app: AppHandle,
    store: State<Store>,
    templates: State<SearchTemplates>,
    input: SourceInput,
) -> Result<SavedSource, String> {
    let url = catalog::check_url(&input.url)?;
    let id = input.id.filter(|id| !id.is_empty()).unwrap_or_else(|| format!("{:x}", clock::now_nanos()));
    let name = match input.name.trim() {
        "" => url.host_str().unwrap_or("Catalog").to_string(),
        name => name.to_string(),
    };
    let mut source = catalog::Source { id: id.clone(), name, url: url.to_string(), user: input.user.trim().to_string(), sync: input.sync };
    // The reader types the server's address; the catalog may be under /opds.
    // The new password is tried first and kept only if the server takes it.
    let password = input.password.filter(|p| !p.is_empty());
    let url = catalog::discover(&source, catalog::login_with(&source, password.as_deref()).as_ref())?;
    source.url = url.to_string();
    if let Some(password) = password {
        platform::save_password(&id, &password)?;
    }
    store
        .update(|s| {
            if source.sync {
                s.sources.iter_mut().for_each(|other| other.sync = false);
            }
            match s.sources.iter_mut().find(|other| other.id == source.id) {
                Some(existing) => *existing = source,
                None => s.sources.push(source),
            }
        })
        .map_err(|e| e.to_string())?;
    templates.0.lock().unwrap().remove(&id);
    let _ = app.emit("catalogs-changed", ());
    Ok(SavedSource { id, warning: catalog::insecure_warning(&url) })
}

#[tauri::command(async)]
fn delete_catalog_source(app: AppHandle, store: State<Store>, id: String) -> Result<(), String> {
    store.update(|s| s.sources.retain(|source| source.id != id)).map_err(|e| e.to_string())?;
    platform::forget_password(&id);
    let _ = app.emit("catalogs-changed", ());
    Ok(())
}

fn source_by_id(store: &Store, id: &str) -> Result<catalog::Source, String> {
    store.read(|s| s.sources.iter().find(|source| source.id == id).cloned()).ok_or_else(|| "That catalog was removed".into())
}

/// A page of a catalog: its root, or `url` on its server.
#[tauri::command(async)]
fn catalog_feed(store: State<Store>, id: String, url: Option<String>) -> Result<opds::Feed, String> {
    catalog::feed(&source_by_id(&store, &id)?, url.as_deref())
}

#[tauri::command(async)]
fn catalog_search(store: State<Store>, templates: State<SearchTemplates>, id: String, query: String) -> Result<opds::Feed, String> {
    let source = source_by_id(&store, &id)?;
    let known = templates.0.lock().unwrap().get(&id).cloned();
    let template = match known {
        Some(template) => template,
        None => {
            let template = catalog::search_template(&source)?;
            templates.0.lock().unwrap().insert(id, template.clone());
            template
        }
    };
    catalog::search(&source, &template, &query)
}

#[tauri::command(async)]
fn catalog_image(store: State<Store>, id: String, url: String) -> Result<Response, String> {
    catalog::image(&source_by_id(&store, &id)?, &url).map(Response::new)
}

/// Downloads a book from a catalog into the library, under the naming
/// template; a book the library already holds isn't added twice.
#[tauri::command(async)]
fn catalog_download(
    app: AppHandle,
    store: State<Store>,
    books: State<LibraryBooks>,
    id: String,
    url: String,
    format: opds::FileFormat,
) -> Result<Added, String> {
    let source = source_by_id(&store, &id)?;
    let downloads = store.dir().join(CACHE_DIR).join("downloads");
    let file = catalog::download(&source, &url, format, &downloads)?;
    let result = put_in_library(&store, &books, &file, AddMode::Move);
    // Left behind when the library already had the book, or on a failure.
    let _ = std::fs::remove_file(&file);
    let (added, _) = result?;
    let _ = app.emit("library-updated", ());
    Ok(added)
}

/// Points recent-list entries at the books' new places (by fingerprint), in one write.
fn repath_recent(store: &Store, moves: &[(String, PathBuf)]) -> Result<(), String> {
    store
        .update(|s| {
            for (fingerprint, path) in moves {
                s.recent.iter_mut().filter(|r| &r.fingerprint == fingerprint).for_each(|r| r.path = path.clone());
            }
        })
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn get_bookmarks(store: State<Store>, fingerprint: String) -> Value {
    store.read(|s| s.bookmarks.get(&fingerprint).cloned().unwrap_or_else(|| Value::Array(vec![])))
}

/// Replaces a book's bookmarks (an empty list forgets the book).
#[tauri::command(async)]
fn set_bookmarks(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    fingerprint: String,
    bookmarks: Value,
) -> Result<(), String> {
    saving_window(&windows, &window)?;
    store
        .update(|s| match bookmarks.as_array() {
            Some(list) if !list.is_empty() => {
                s.bookmarks.insert(fingerprint, bookmarks);
            }
            _ => {
                s.bookmarks.remove(&fingerprint);
            }
        })
        .map_err(|e| e.to_string())
}

/// "Never for this file": stops offering this book for the library.
#[tauri::command(async)]
fn decline_library(store: State<Store>, fingerprint: String) -> Result<(), String> {
    store
        .update(|s| {
            s.declined.insert(fingerprint);
        })
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct Rename {
    from: String,
    to: String,
}

/// The renames that would make the last scan's books follow the naming template.
#[tauri::command(async)]
fn organize_preview(store: State<Store>, books: State<LibraryBooks>) -> Result<Vec<Rename>, String> {
    let root = library_root(&store)?;
    let plan = library::organize_plan(&root, &books.snapshot(&store, &root)).map_err(|e| e.to_string())?;
    Ok(plan.into_iter().map(|(entry, to)| Rename { from: entry.path, to }).collect())
}

/// Applies the plan (recomputed here, so the frontend can't name arbitrary
/// files). Returns how many books were renamed.
#[tauri::command(async)]
fn organize_apply(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, windows: State<ReaderWindows>) -> Result<usize, String> {
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

/// Brings the library window forward, opening it if needed; on a phone,
/// shows the library in its one window.
/// Async: on Windows, building a window in a sync command (which runs on the
/// main thread) deadlocks.
#[tauri::command(async)]
fn show_library(window: Window) -> Result<(), String> {
    if cfg!(mobile) {
        show_here(&window, None, false);
        return Ok(());
    }
    open_or_focus(window.app_handle(), Launch::default()).map_err(|e| e.to_string())
}

/// Back on the library screen of a phone: the app goes to the background,
/// as Android apps do. Nothing happens on the desktop.
#[tauri::command(async)]
fn leave_app(app: AppHandle) -> Result<(), String> {
    platform::leave(&app)
}

/// Blocks the WebView context menu in every frame (the app has no editing),
/// except in edit fields where the menu stays useful.
const NO_CONTEXT_MENU_SCRIPT: &str =
    "document.addEventListener('contextmenu', e => { if (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable]')) return; e.preventDefault(); });";

/// foliate-js's paginator re-lays pages out from a ResizeObserver, and the
/// browser then reports "ResizeObserver loop completed with undelivered
/// notifications". It is harmless (the rest arrives next frame), so it is
/// dropped before any other listener (such as Vite's dev overlay) sees it.
/// It has to be an initialization script: listeners on `window` run in the
/// order they were added, and this runs before the page's own scripts.
const QUIET_RESIZE_OBSERVER_SCRIPT: &str =
    "addEventListener('error', e => { if (String(e.message).includes('ResizeObserver loop')) { e.preventDefault(); e.stopImmediatePropagation(); } });";

fn open_window(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    let label = format!("reader-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let first = app.webview_windows().is_empty();
    app.state::<ReaderWindows>().insert(&label, launch);

    let theme = app.state::<Store>().read(|s| s.settings.get("theme").cloned());
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::default())
        .initialization_script(theme_script(theme.as_ref().and_then(|t| t.as_str())))
        .initialization_script_for_all_frames(NO_CONTEXT_MENU_SCRIPT)
        .initialization_script(QUIET_RESIZE_OBSERVER_SCRIPT);
    #[cfg(desktop)]
    return place_on_desktop(app, builder, first);
    // A phone has one full-screen window.
    #[cfg(mobile)]
    {
        let _ = first;
        builder.build().map(|_| ())
    }
}

/// Sizes and places a desktop window: the first one where the last window
/// was (if that is still on a screen), later ones cascaded by Windows.
#[cfg(desktop)]
fn place_on_desktop(app: &AppHandle, builder: WebviewWindowBuilder<tauri::Wry, AppHandle>, first: bool) -> tauri::Result<()> {
    let geometry = app.state::<Store>().read(|s| s.window.clone());
    let mut builder = builder.title("Book Reader").min_inner_size(400.0, 300.0).visible(false);
    builder = match &geometry {
        Some(g) if first => builder.inner_size(g.width, g.height).position(g.x, g.y).maximized(g.maximized),
        // Windows places (and cascades) later windows itself.
        Some(g) => builder.inner_size(g.width, g.height),
        None => builder.inner_size(DEFAULT_WIDTH, DEFAULT_HEIGHT).center(),
    };
    let window = builder.build()?;
    if first && geometry.is_some() && !is_on_screen(&window)? {
        window.center()?;
    }
    window.show()
}

/// Sets the saved theme before the first paint so the window never flashes
/// the wrong colours; the frontend takes over once its settings load.
fn theme_script(theme: Option<&str>) -> String {
    let resolved = match theme {
        Some(t @ ("light" | "dark" | "sepia")) => format!("'{t}'"),
        _ => "matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'".into(),
    };
    // The script may run before <html> exists; then wait for it.
    format!(
        "(() => {{
            const apply = () => !!document.documentElement && !!(document.documentElement.dataset.theme = {resolved});
            if (!apply()) new MutationObserver((_, o) => apply() && o.disconnect()).observe(document, {{ childList: true }});
        }})();"
    )
}

/// A saved position can point at a monitor that is no longer connected.
#[cfg(desktop)]
fn is_on_screen(window: &tauri::WebviewWindow) -> tauri::Result<bool> {
    let pos = window.outer_position()?;
    Ok(window.monitor_from_point(pos.x.into(), pos.y.into())?.is_some())
}

#[cfg(desktop)]
fn remember_geometry(window: &Window) -> tauri::Result<()> {
    if window.is_fullscreen()? {
        return Ok(());
    }
    let maximized = window.is_maximized()?;
    let scale = window.scale_factor()?;
    let pos = window.outer_position()?.to_logical::<f64>(scale);
    let size = window.inner_size()?.to_logical::<f64>(scale);
    let store = window.state::<Store>();
    store.update(|s| {
        // A maximized window keeps the size it will restore to.
        s.window = match (&s.window, maximized) {
            (Some(prev), true) => Some(WindowGeometry { maximized, ..prev.clone() }),
            _ => Some(WindowGeometry { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized }),
        };
    })?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The desktop app takes a book and options on the command line.
    #[cfg(desktop)]
    let (launch, data_dir) = {
        let cwd = std::env::current_dir().unwrap_or_default();
        match cli::parse(std::env::args().skip(1), &cwd) {
            Command::Open { launch, data_dir } => (launch, data_dir),
            other => std::process::exit(run_command(other)),
        }
    };
    #[cfg(mobile)]
    let launch = Launch::default();

    let builder = tauri::Builder::default();
    // Must come first: a second launch hands its arguments to this process and exits.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        if let Command::Open { launch, .. } = cli::parse(argv.into_iter().skip(1), Path::new(&cwd)) {
            let _ = open_or_focus(app, launch);
        }
    }));
    platform::plugins(builder)
        .plugin(tauri_plugin_opener::init())
        .manage(ReaderWindows::default())
        .manage(LibraryBooks::default())
        .manage(SearchTemplates::default())
        .manage(Originals::default())
        .invoke_handler(tauri::generate_handler![
            current_book,
            read_book,
            save_position,
            get_settings,
            update_settings,
            remember_book,
            recent_books,
            open_recent,
            forget_recent,
            clear_recent,
            register_file_types,
            unregister_file_types,
            file_types_registered,
            app_version,
            open_repository,
            check_for_update,
            open_latest_release,
            open_incoming,
            delete_library_book,
            set_reading,
            pick_book,
            library_info,
            set_library,
            scan_library,
            library_cover,
            open_library_book,
            show_library,
            leave_app,
            catalog_sources,
            save_catalog_source,
            delete_catalog_source,
            catalog_feed,
            catalog_search,
            catalog_image,
            catalog_download,
            sync_document,
            sync_pull,
            sync_push,
            add_to_library,
            decline_library,
            get_bookmarks,
            set_bookmarks,
            organize_preview,
            organize_apply,
        ])
        .on_window_event(|window, event| match event {
            #[cfg(desktop)]
            WindowEvent::CloseRequested { .. } => {
                let _ = remember_geometry(window);
            }
            WindowEvent::Destroyed => {
                window.state::<ReaderWindows>().remove(window.label());
            }
            #[cfg(desktop)]
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                if let Some(path) = paths.first() {
                    show_book(window, path.clone(), false);
                }
            }
            _ => {}
        })
        .setup(move |app| {
            // Loaded here, in the one running app: a second launch hands over
            // its file and exits before setup, so it never touches the data file.
            // On Android the app's private storage keeps the state.
            #[cfg(mobile)]
            let data_dir = Some(app.path().app_data_dir()?);
            app.manage(Store::load(data_dir::resolve_state_file(data_dir.as_deref())));
            open_window(app.handle(), launch)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Book Reader");
}

/// Runs a command that doesn't open a window; returns the exit code.
#[cfg(desktop)]
fn run_command(command: Command) -> i32 {
    match command {
        Command::Help => report(Ok(()), cli::USAGE),
        Command::Version => report(Ok(()), &version::text(tauri::webview_version().ok().as_deref())),
        Command::Invalid(message) => {
            console::print(&format!("{message}\n\n{}", cli::USAGE));
            2
        }
        Command::Register => report(
            std::env::current_exe().and_then(|exe| register::register(&exe)),
            &format!(
                "Book Reader is registered for {}.\nIf another app still opens them, right-click a file > Open with > \
                 Choose another app > Book Reader, and tick \"Always\".",
                register::EXTENSIONS.join(", ")
            ),
        ),
        Command::Unregister => report(register::unregister(), "Book Reader is no longer registered for any file types."),
        Command::Open { .. } => unreachable!("opening is handled by run()"),
    }
}

#[cfg(desktop)]
fn report(result: std::io::Result<()>, success: &str) -> i32 {
    match result {
        Ok(()) => {
            console::print(success);
            0
        }
        Err(e) => {
            console::print(&format!("Failed: {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{compare_positions, kosync::Progress, theme_script, Position};

    fn here(cfi: &str, time: u64) -> Position {
        Position { cfi: cfi.into(), fraction: 0.5, time }
    }

    fn server(cfi: &str, device_id: &str, timestamp: u64) -> Progress {
        Progress { progress: cfi.into(), percentage: 0.6, device: "phone".into(), device_id: device_id.into(), timestamp }
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

    #[test]
    fn theme_script_uses_known_themes() {
        assert!(theme_script(Some("sepia")).contains("dataset.theme = 'sepia'"));
    }

    #[test]
    fn theme_script_falls_back_to_the_system_theme() {
        for theme in [None, Some("system"), Some("'; alert(1); '")] {
            let script = theme_script(theme);
            assert!(script.contains("prefers-color-scheme"), "{script}");
            assert!(!script.contains("alert"), "{script}");
        }
    }
}
