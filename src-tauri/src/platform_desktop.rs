//! Where the library lives on the desktop (see platform_android.rs for the phone).

use crate::{data_dir, file_dialog};
use std::path::PathBuf;
use tauri::{AppHandle, Builder, Manager, Window, Wry};

/// The desktop needs no platform plugins.
pub fn plugins(builder: Builder<Wry>) -> Builder<Wry> {
    builder
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

/// The desktop has no Back button, so there is nothing to leave.
pub fn leave(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}

/// Any folder the reader chose can be read.
pub fn check_library_access(_app: &AppHandle) -> Result<(), String> {
    Ok(())
}
