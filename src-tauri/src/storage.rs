//! Android: the library lives in shared storage, reached with All files access
//! through `StoragePlugin.kt`. With the access on, the library is a plain
//! folder and the rest of the app (scanning, Move / Copy, Organize) works on
//! it as on Windows.

use serde::Deserialize;
use std::path::PathBuf;
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{AppHandle, Manager, Runtime, Wry};

/// The library folder offered on first run, inside the shared Documents folder.
const LIBRARY_FOLDER: &str = "Books";

/// The handle to `StoragePlugin.kt`, kept in the app's state.
pub struct Storage<R: Runtime>(PluginHandle<R>);

/// The plugin, from anywhere with the app.
pub fn of(app: &AppHandle) -> tauri::State<'_, Storage<Wry>> {
    app.state()
}

/// Registers `StoragePlugin.kt` and keeps its handle in the app's state.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("storage")
        .setup(|app, api| {
            app.manage(Storage(api.register_android_plugin("com.olegg90.bookreader", "StoragePlugin")?));
            Ok(())
        })
        .build()
}

#[derive(Deserialize)]
struct Status {
    granted: bool,
    documents: PathBuf,
}

impl<R: Runtime> Storage<R> {
    fn status(&self) -> Result<Status, String> {
        self.0.run_mobile_plugin("status", ()).map_err(|e| e.to_string())
    }

    /// `Documents/Books` in shared storage.
    pub fn default_library(&self) -> Result<PathBuf, String> {
        Ok(self.status()?.documents.join(LIBRARY_FOLDER))
    }

    /// Fails while All files access is off, saying where to turn it on.
    pub fn check_access(&self) -> Result<(), String> {
        if self.status()?.granted {
            return Ok(());
        }
        Err("Book Reader can't read the library: turn on \"Allow access to manage all files\" for it in Android's settings.".into())
    }

    /// Like `check_access`, and also opens Android's setting for it.
    pub fn require_access(&self) -> Result<(), String> {
        if self.check_access().is_ok() {
            return Ok(());
        }
        self.0.run_mobile_plugin::<serde_json::Value>("askAllFilesAccess", ()).map_err(|e| e.to_string())?;
        Err("Turn on \"Allow access to manage all files\" for Book Reader, come back and tap again.".into())
    }
}
