//! One-off system commands: file types, updates, outside books, the library screen.

use crate::cli::Launch;
use crate::cloud::CloudStore;
use crate::dropbox;
use crate::google;
use crate::formats;
use crate::modules;
use crate::platform::{Active, Platform};
use crate::store::Store;
use crate::register;
use crate::updates;
use crate::windows::{open_or_focus, show_book, show_here};
use crate::CmdResult;
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State, Window};
use tauri_plugin_opener::OpenerExt;

/// Registers the app for its book file types (see register::register).
#[tauri::command(async)]
pub(crate) fn register_file_types(store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::FILE_TYPES))?;
    std::env::current_exe().and_then(|exe| register::register(&exe)).map_err(|e| e.to_string())
}

/// Removes the app's file associations again.
#[tauri::command(async)]
pub(crate) fn unregister_file_types(store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::FILE_TYPES))?;
    register::unregister().map_err(|e| e.to_string())
}

/// Whether the app is registered for its book file types.
#[tauri::command(async)]
pub(crate) fn file_types_registered() -> bool {
    register::registered()
}

/// A cloud's connection for the settings: whether a refresh token is kept,
/// and the last sync failure, if any (sync stays quiet, but the settings
/// show what went wrong).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudConnection {
    connected: bool,
    last_error: Option<String>,
}

/// A cloud's connection as the settings show it.
fn cloud_connection(store: &dyn CloudStore) -> CloudConnection {
    CloudConnection { connected: store.connected(), last_error: store.errors().last() }
}

/// Dropbox's connection for the settings.
#[tauri::command(async)]
pub(crate) fn dropbox_status() -> CloudConnection {
    cloud_connection(&dropbox::STORE)
}

/// Connects Dropbox: opens its login in the browser, waits for the answer
/// and keeps the refresh token like a catalog password.
#[tauri::command(async)]
pub(crate) fn dropbox_connect(app: AppHandle, store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::DROPBOX))?;
    let pending = dropbox::begin()?;
    app.opener().open_url(&pending.url, None::<&str>).map_err(|e| e.to_string())?;
    dropbox::finish(pending)
}

/// Forgets the Dropbox token; connecting again starts over. Works with the
/// module off too, so a switched-off cloud never keeps its token.
#[tauri::command(async)]
pub(crate) fn dropbox_disconnect() -> CmdResult<()> {
    crate::cloud::disconnect(&dropbox::STORE);
    Ok(())
}

/// Google Drive's connection for the settings.
#[tauri::command(async)]
pub(crate) fn google_status() -> CloudConnection {
    cloud_connection(&google::STORE)
}

/// Connects Google Drive: opens its login in the browser, waits for the answer
/// and keeps the refresh token like a catalog password.
#[tauri::command(async)]
pub(crate) fn google_connect(app: AppHandle, store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::GOOGLE_DRIVE))?;
    let pending = google::begin()?;
    app.opener().open_url(&pending.url, None::<&str>).map_err(|e| e.to_string())?;
    google::finish(pending)
}

/// Forgets the Google token; connecting again starts over. Works with the
/// module off too, so a switched-off cloud never keeps its token.
#[tauri::command(async)]
pub(crate) fn google_disconnect() -> CmdResult<()> {
    google::disconnect();
    Ok(())
}

/// Starts the phone's Google login: opens its page in the browser, and the
/// answer comes back through the app's intent filter (`google_finish`).
/// The desktop waits on a loopback address instead (`google_connect`).
#[tauri::command(async)]
pub(crate) fn google_connect_android(app: AppHandle, store: State<Store>) -> CmdResult<String> {
    store.read(|s| modules::require(s, modules::id::GOOGLE_DRIVE))?;
    let url = google::begin_android()?;
    app.opener().open_url(&url, None::<&str>).map_err(|e| e.to_string())?;
    Ok(url)
}

/// Finishes the phone's Google login with the address the browser came back to.
#[tauri::command(async)]
pub(crate) fn google_finish(answer: String, store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::GOOGLE_DRIVE))?;
    google::finish_answer(&answer)
}

/// A Google sign-in answer waiting on a phone, if one arrived. The settings drain
/// it at startup and on every oauth event, so a cold start cannot lose it.
#[tauri::command(async)]
pub(crate) fn google_take_answer() -> CmdResult<Option<String>> {
    Active::take_oauth_answer()
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
pub(crate) fn open_latest_release(app: AppHandle, store: State<Store>) -> CmdResult<()> {
    store.read(|s| modules::require(s, modules::id::UPDATE_CHECK))?;
    app.opener().open_url(format!("{}/releases/latest", updates::REPOSITORY), None::<&str>).map_err(|e| e.to_string())
}

/// Asks GitHub for the latest release and compares it with this copy.
#[tauri::command(async)]
pub(crate) fn check_for_update(store: State<Store>) -> CmdResult<updates::UpdateCheck> {
    store.read(|s| modules::require(s, modules::id::UPDATE_CHECK))?;
    updates::check()
}

/// Ctrl+O (on a phone, "Add book…"): asks for a book and opens it in this window.
#[tauri::command(async)]
pub(crate) fn pick_book(window: Window, originals: State<Originals>, store: State<Store>) -> CmdResult<()> {
    // The dialog offers only the enabled formats; a stale pick still refuses below.
    let enabled = store.read(|s| formats::enabled_extensions(&modules::disabled_ids(s)));
    if let Some(book) = Active::pick_book(&window, &enabled)? {
        if let Some(reader) = formats::of(&book.path) {
            store.read(|s| formats::check(s, reader))?;
        }
        open_outside(&window, &originals, book);
    }
    Ok(())
}

/// Opens a book handed to the app ("Open with", "Share" on a phone), if one waits.
#[tauri::command(async)]
pub(crate) fn open_incoming(window: Window, originals: State<Originals>) -> CmdResult<()> {
    if let Some(book) = Active::take_incoming()? {
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
    Active::set_reading(on, volume_keys)
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
    Active::leave(&app)
}
