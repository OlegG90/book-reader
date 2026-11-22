//! Where the library lives on the desktop (see platform_android.rs for the phone).

use crate::{data_dir, Outside};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder};
use std::path::PathBuf;
use tauri::{AppHandle, Builder, Manager, Window, Wry};

/// File dialogs (books, the library folder).
pub fn plugins(builder: Builder<Wry>) -> Builder<Wry> {
    builder.plugin(tauri_plugin_dialog::init())
}

/// A file dialog, modal to `window`.
fn file_dialog(window: &Window) -> FileDialogBuilder<Wry> {
    window.dialog().file().set_parent(window)
}

/// What the first-run screen offers: `lib` next to the exe, or `Documents\Book Reader`.
pub fn suggested_library(_app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir::resolve_default_library())
}

/// The suggested folder, or one the reader picks (`None` when cancelled).
pub fn chosen_library(window: &Window, pick: bool) -> Result<Option<PathBuf>, String> {
    if !pick {
        return suggested_library(window.app_handle()).map(Some);
    }
    file_dialog(window).blocking_pick_folder().map(|f| f.into_path().map_err(|e| e.to_string())).transpose()
}

/// Ctrl+O: a book file the reader picks (`None` when cancelled).
pub fn pick_book(window: &Window) -> Result<Option<Outside>, String> {
    let picked = file_dialog(window).add_filter("Books", &["epub", "fb2", "fb2.zip", "fbz"]).blocking_pick_file();
    picked.map(|f| f.into_path().map(|path| Outside { path, original: None }).map_err(|e| e.to_string())).transpose()
}

/// Books are handed to the desktop app on its command line instead.
pub fn take_incoming() -> Result<Option<Outside>, String> {
    Ok(None)
}

/// Desktop books are moved as files, so there is no original to delete.
pub fn delete_original(_original: &str) -> bool {
    false
}

/// The desktop has no screen timeout or status bar to manage while reading.
pub fn set_reading(_on: bool) -> Result<(), String> {
    Ok(())
}

/// The desktop has no volume keys to turn pages with.
pub fn set_volume_keys(_on: bool) -> Result<(), String> {
    Ok(())
}

/// The credential store entry for a catalog's password.
fn credential(source_id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("Book Reader", &format!("catalog:{source_id}")).map_err(|e| e.to_string())
}

/// Keeps a catalog's password in the OS credential store (the Windows Credential Manager).
pub fn save_password(source_id: &str, password: &str) -> Result<(), String> {
    credential(source_id)?.set_password(password).map_err(|e| e.to_string())
}

/// A catalog's password, if one is kept.
pub fn password(source_id: &str) -> Option<String> {
    credential(source_id).ok()?.get_password().ok()
}

/// Removes a catalog's password from the credential store.
pub fn forget_password(source_id: &str) {
    if let Ok(entry) = credential(source_id) {
        let _ = entry.delete_credential();
    }
}

/// The desktop has no Back button, so there is nothing to leave.
pub fn leave(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

/// Any folder the reader chose can be read.
pub fn check_library_access(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}
