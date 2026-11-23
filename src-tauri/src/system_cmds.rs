//! One-off system commands: file types, updates, outside books, the library screen.

use crate::cli::Launch;
use crate::platform;
use crate::register;
use crate::updates;
use crate::windows::{open_or_focus, show_book, show_here};
use crate::CmdResult;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State, Window};
use tauri_plugin_opener::OpenerExt;

/// Registers the app for its book file types (see register::register).
#[tauri::command(async)]
pub(crate) fn register_file_types() -> CmdResult<()> {
    std::env::current_exe().and_then(|exe| register::register(&exe)).map_err(|e| e.to_string())
}

/// Removes the app's file associations again.
#[tauri::command(async)]
pub(crate) fn unregister_file_types() -> CmdResult<()> {
    register::unregister().map_err(|e| e.to_string())
}

/// Whether the app is registered for its book file types.
#[tauri::command(async)]
pub(crate) fn file_types_registered() -> bool {
    register::registered()
}

/// The app version (from Cargo.toml), shown in the About panel.
#[tauri::command]
pub(crate) fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Opens the app's repository on GitHub in the browser (Settings → About).
#[tauri::command]
pub(crate) fn open_repository(app: AppHandle) -> CmdResult<()> {
    app.opener().open_url(updates::REPOSITORY, None::<&str>).map_err(|e| e.to_string())
}

/// Opens the latest release's page, where its files are, in the browser.
#[tauri::command]
pub(crate) fn open_latest_release(app: AppHandle) -> CmdResult<()> {
    app.opener().open_url(format!("{}/releases/latest", updates::REPOSITORY), None::<&str>).map_err(|e| e.to_string())
}

/// Asks GitHub for the latest release and compares it with this copy.
#[tauri::command(async)]
pub(crate) fn check_for_update() -> CmdResult<updates::UpdateCheck> {
    updates::check()
}

/// Ctrl+O (on a phone, "Add book…"): asks for a book and opens it in this window.
#[tauri::command(async)]
pub(crate) fn pick_book(window: Window, originals: State<Originals>) -> CmdResult<()> {
    if let Some(book) = platform::pick_book(&window)? {
        open_outside(&window, &originals, book);
    }
    Ok(())
}

/// Opens a book handed to the app ("Open with", "Share" on a phone), if one waits.
#[tauri::command(async)]
pub(crate) fn open_incoming(window: Window, originals: State<Originals>) -> CmdResult<()> {
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
pub(crate) struct Originals(pub(crate) Mutex<HashMap<PathBuf, String>>);

fn open_outside(window: &Window, originals: &Originals, book: Outside) {
    if let Some(original) = book.original {
        originals.0.lock().unwrap().insert(book.path.clone(), original);
    }
    show_book(window, book.path, false);
}

/// A book is open (or not): on a phone the screen stays on, the status bar
/// hides, and the volume keys turn pages if that setting is on.
#[tauri::command(async)]
pub(crate) fn set_reading(on: bool, volume_keys: bool) -> CmdResult<()> {
    platform::set_reading(on, volume_keys)
}

/// Brings the library window forward, opening it if needed; on a phone,
/// shows the library in its one window.
/// Async: on Windows, building a window in a sync command (which runs on the
/// main thread) deadlocks.
#[tauri::command(async)]
pub(crate) fn show_library(window: Window) -> CmdResult<()> {
    if cfg!(mobile) {
        show_here(&window, None, false);
        return Ok(());
    }
    open_or_focus(window.app_handle(), Launch::default()).map_err(|e| e.to_string())
}

/// Back on the library screen of a phone: the app goes to the background,
/// as Android apps do. Nothing happens on the desktop.
#[tauri::command(async)]
pub(crate) fn leave_app(app: AppHandle) -> CmdResult<()> {
    platform::leave(&app)
}
