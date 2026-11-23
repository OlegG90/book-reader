//! The library folder: what it holds, and moving books into it.

use crate::library;
use crate::metadata;
use crate::naming;
use crate::paths;
use crate::platform::{Active, Platform};
use crate::store::Store;
use crate::system_cmds::Originals;
use crate::windows::{saving_window, show_book, ReaderWindows};
use crate::CmdResult;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State, Window, ipc::Response};

/// The books found by the last scan. The frontend names books by their key
/// (the path inside the library), which is only accepted when the scan found
/// it, so the frontend still cannot make the app open arbitrary files.
#[derive(Default)]
pub(crate) struct LibraryBooks(Mutex<Vec<library::Entry>>);

impl LibraryBooks {
    pub(crate) fn entry(&self, key: &str) -> CmdResult<library::Entry> {
        let books = self.0.lock().unwrap();
        books.iter().find(|b| b.path == key).cloned().ok_or_else(|| "That book is no longer in the library".into())
    }

    pub(crate) fn path_of(&self, store: &Store, key: &str) -> CmdResult<PathBuf> {
        Ok(self.entry(key)?.absolute(&library_root(store)?))
    }

    /// Takes a deleted book out of the list.
    pub(crate) fn forget(&self, key: &str) {
        self.0.lock().unwrap().retain(|b| b.path != key);
    }

    /// The last scan's books; before any scan in this run, the cached ones.
    pub(crate) fn snapshot(&self, store: &Store, root: &Path) -> Vec<library::Entry> {
        let books = self.0.lock().unwrap();
        if books.is_empty() { library_cache(store).load(root) } else { books.clone() }
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
    books.0.lock().unwrap().clear();
    let _ = window.emit("library-changed", ());
    Ok(Some(folder.display().to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryBook {
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
pub(crate) fn scan_library(app: AppHandle, store: State<Store>, books: State<LibraryBooks>) -> CmdResult<Vec<LibraryBook>> {
    let root = library_root(&store)?;
    Active::check_library_access(&app)?;
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
pub(crate) fn library_cover(store: State<Store>, books: State<LibraryBooks>, key: String) -> CmdResult<Response> {
    // Before the first scan the history's covers come from the cache, as recent_books found them:
    // the snapshot is the last scan's books, or the cached ones.
    let root = library_root(&store)?;
    let cover = books.snapshot(&store, &root).into_iter().find(|b| b.path == key).and_then(|b| b.cover).ok_or("No cover")?;
    std::fs::read(library_cache(&store).covers().join(cover)).map(Response::new).map_err(|e| e.to_string())
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
    show_book(&window, books.path_of(&store, &key)?, from_start);
    Ok(())
}

/// Deletes a book from the library: the file goes to the Recycle Bin (a phone
/// deletes it). Its reading position and bookmarks stay, by its content, so
/// the book picks up where it was if it comes back; the history forgets it.
#[tauri::command(async)]
pub(crate) fn delete_library_book(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, key: String) -> CmdResult<()> {
    let entry = books.entry(&key)?;
    let path = entry.absolute(&library_root(&store)?);
    Active::delete_book(&path).map_err(|e| format!("{}: {e}", paths::file_name(&path)))?;
    books.forget(&key);
    if store.read(|s| s.recent.iter().any(|r| r.fingerprint == entry.fingerprint)) {
        store.update(|s| s.recent.retain(|r| r.fingerprint != entry.fingerprint)).map_err(|e| e.to_string())?;
    }
    let _ = app.emit("library-updated", ());
    Ok(())
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Added {
    /// Where the book is now, inside the library.
    path: String,
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
    let root = library_root(&store)?;
    let plan = library::organize_plan(&root, &books.snapshot(&store, &root)).map_err(|e| e.to_string())?;
    Ok(plan.into_iter().map(|(entry, to)| Rename { from: entry.path, to }).collect())
}

/// Applies the plan (recomputed here, so the frontend can't name arbitrary
/// files). Returns how many books were renamed.
#[tauri::command(async)]
pub(crate) fn organize_apply(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, windows: State<ReaderWindows>) -> CmdResult<usize> {
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
