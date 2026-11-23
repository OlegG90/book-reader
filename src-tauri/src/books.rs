//! The books around the reader: the window's book, positions, the recent list.

use crate::cli::Launch;
use crate::clock::now_ms;
use crate::fingerprint;
use crate::library_cmds::{library_root, LibraryBooks};
use crate::paths;
use crate::store::{Position, RecentBook, Store};
use crate::windows::{saving_window, show_book, ReaderWindows};
use crate::CmdResult;
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use tauri::{Emitter, Manager, State, Window, ipc::Response};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BookInfo {
    file_name: String,
    fingerprint: String,
    position: Option<Position>,
    no_save: bool,
    offer: Option<Offer>,
}

/// What the bar under a book offers.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Offer {
    /// The book lies outside the library: copy or move it in.
    Add,
    /// No library folder has been chosen yet (the app was first started on a book).
    Setup,
}

#[tauri::command(async)]
pub(crate) fn current_book(window: Window, windows: State<ReaderWindows>, store: State<Store>) -> CmdResult<Option<BookInfo>> {
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
pub(crate) fn read_book(window: Window, windows: State<ReaderWindows>) -> CmdResult<Response> {
    let path = windows.path_of(window.label())?;
    std::fs::read(path).map(Response::new).map_err(|e| e.to_string())
}

/// Takes the fingerprint from the frontend so a position that is still
/// pending when another book replaces it is saved under the right book.
#[tauri::command(async)]
pub(crate) fn save_position(store: State<Store>, fingerprint: String, mut position: Position) -> CmdResult<()> {
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
pub(crate) fn get_settings(store: State<Store>) -> Map<String, Value> {
    store.read(|s| s.settings.clone())
}

/// Merges changed settings (so windows never overwrite each other's changes)
/// and passes them on to the other windows.
#[tauri::command(async)]
pub(crate) fn update_settings(window: Window, store: State<Store>, changes: Map<String, Value>) -> CmdResult<()> {
    store.update(|s| s.settings.extend(changes.clone())).map_err(|e| e.to_string())?;
    for label in window.app_handle().webview_windows().into_keys().filter(|l| l != window.label()) {
        let _ = window.emit_to(&label, "settings-changed", &changes);
    }
    Ok(())
}

/// Adds the window's book to the recent list, with the title and author the
/// frontend read from it.
#[tauri::command(async)]
pub(crate) fn remember_book(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    fingerprint: String,
    title: String,
    author: String,
) -> CmdResult<()> {
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
pub(crate) struct RecentView {
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
pub(crate) fn recent_books(store: State<Store>, library: State<LibraryBooks>) -> Vec<RecentView> {
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
pub(crate) fn open_recent(window: Window, store: State<Store>, key: String) -> CmdResult<()> {
    let path = store
        .read(|s| s.recent.iter().find(|r| r.fingerprint == key).map(|r| r.path.clone()))
        .ok_or("That book is no longer in the recent list")?;
    show_book(&window, path, false);
    Ok(())
}

#[tauri::command(async)]
pub(crate) fn forget_recent(store: State<Store>, key: String) -> CmdResult<()> {
    store.update(|s| s.recent.retain(|r| r.fingerprint != key)).map_err(|e| e.to_string())
}

/// Forgets the recent books list. Reading positions are kept, so reopened books resume.
#[tauri::command(async)]
pub(crate) fn clear_recent(store: State<Store>) -> CmdResult<()> {
    store.update(|s| s.recent.clear()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub(crate) fn get_bookmarks(store: State<Store>, fingerprint: String) -> Value {
    store.read(|s| s.bookmarks.get(&fingerprint).cloned().unwrap_or_else(|| Value::Array(vec![])))
}

/// Replaces a book's bookmarks (an empty list forgets the book).
#[tauri::command(async)]
pub(crate) fn set_bookmarks(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    fingerprint: String,
    bookmarks: Value,
) -> CmdResult<()> {
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
pub(crate) fn decline_library(store: State<Store>, fingerprint: String) -> CmdResult<()> {
    store
        .update(|s| {
            s.declined.insert(fingerprint);
        })
        .map_err(|e| e.to_string())
}
