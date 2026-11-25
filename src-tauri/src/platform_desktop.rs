//! Where the library lives on the desktop (see platform_android.rs for the phone).

use super::Platform;
use crate::data_dir;
use crate::system_cmds::Outside;
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Builder, Manager, Window, Wry};

/// The desktop side of the platform contract.
pub(crate) struct Desktop;

impl Platform for Desktop {
    /// File dialogs (books, the library folder).
    fn plugins(builder: Builder<Wry>) -> Builder<Wry> {
        builder.plugin(tauri_plugin_dialog::init())
    }

    /// What the first-run screen offers: `lib` next to the exe, or `Documents\Book Reader`.
    fn suggested_library(_app: &AppHandle) -> Result<PathBuf, String> {
        Ok(data_dir::resolve_default_library())
    }

    /// The suggested folder, or one the reader picks (`None` when cancelled).
    fn chosen_library(window: &Window, pick: bool) -> Result<Option<PathBuf>, String> {
        if !pick {
            return Self::suggested_library(window.app_handle()).map(Some);
        }
        file_dialog(window).blocking_pick_folder().map(|f| f.into_path().map_err(|e| e.to_string())).transpose()
    }

    /// Any folder the reader chose can be read.
    fn check_library_access(_app: &AppHandle) -> Result<(), String> {
        Ok(())
    }

    /// Ctrl+O: a book file the reader picks (`None` when cancelled).
    fn pick_book(window: &Window, extensions: &[&str]) -> Result<Option<Outside>, String> {
        let picked = file_dialog(window).add_filter("Books", extensions).blocking_pick_file();
        picked.map(|f| f.into_path().map(|path| Outside { path, original: None }).map_err(|e| e.to_string())).transpose()
    }

    /// Books are handed to the desktop app on its command line instead.
    fn take_incoming() -> Result<Option<Outside>, String> {
        Ok(None)
    }

    /// A Google sign-in answer waiting on a phone; nothing waits on the desktop.
    fn take_oauth_answer() -> Result<Option<String>, String> {
        Ok(None)
    }

    /// Desktop books are moved as files, so there is no original to delete.
    fn delete_original(_original: &str) -> bool {
        false
    }

    /// Deletes a book file from the library: to the Recycle Bin, so it can come back.
    fn delete_book(path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|e| e.to_string())
    }

    /// The desktop has no screen timeout, status bar or volume keys to manage while reading.
    fn set_reading(_on: bool, _volume_keys: bool) -> Result<(), String> {
        Ok(())
    }

    /// Keeps a catalog's password in the OS credential store (the Windows Credential Manager).
    fn save_password(source_id: &str, password: &str) -> Result<(), String> {
        credential(&format!("catalog:{source_id}"))?.set_password(password).map_err(|e| e.to_string())
    }

    /// Whether a catalog has a password kept.
    fn has_password(source_id: &str) -> bool {
        Self::password(source_id).is_some()
    }

    /// One credential-store lookup per source.
    fn has_passwords(source_ids: &[String]) -> Vec<bool> {
        source_ids.iter().map(|id| Self::has_password(id)).collect()
    }

    /// A catalog's password, if one is kept.
    fn password(source_id: &str) -> Option<String> {
        credential(&format!("catalog:{source_id}")).ok()?.get_password().ok()
    }

    /// Removes a catalog's password from the credential store.
    fn forget_password(source_id: &str) {
        if let Ok(entry) = credential(&format!("catalog:{source_id}")) {
            let _ = entry.delete_credential();
        }
    }

    /// The desktop has no Back button, so there is nothing to leave.
    fn leave(_app: &AppHandle) -> Result<(), String> {
        Ok(())
    }

    fn network_metered() -> bool {
        false
    }
}

/// A file dialog, modal to `window`.
fn file_dialog(window: &Window) -> FileDialogBuilder<Wry> {
    window.dialog().file().set_parent(window)
}

/// The credential store entry for a catalog's password.
/// A credential-store entry by its full account name (`catalog:<id>` for catalogs).
fn credential(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("Book Reader", account).map_err(|e| e.to_string())
}
