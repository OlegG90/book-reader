//! Where the library lives on Android (see platform_desktop.rs for Windows):
//! `Documents/Books` in shared storage, reached with All files access through
//! `SystemPlugin.kt`. With the access on, the library is a plain folder and
//! the rest of the app (scanning, Organize) works on it as on Windows.

use serde::Deserialize;
use std::path::PathBuf;
use tauri::plugin::{self, PluginHandle};
use tauri::{AppHandle, Builder, Manager, Window, Wry};

/// The library folder offered on first run, inside the shared Documents folder.
const LIBRARY_FOLDER: &str = "Books";

/// The handle to `SystemPlugin.kt`, kept in the app's state.
struct System(PluginHandle<Wry>);

/// Registers `SystemPlugin.kt` and keeps its handle in the app's state.
pub fn plugins(builder: Builder<Wry>) -> Builder<Wry> {
    builder.plugin(
        plugin::Builder::<Wry, ()>::new("system")
            .setup(|app, api| {
                app.manage(System(api.register_android_plugin("com.olegg90.bookreader", "SystemPlugin")?));
                Ok(())
            })
            .build(),
    )
}

/// What `SystemPlugin.status` reports.
#[derive(Deserialize)]
struct Status {
    /// All files access is on.
    granted: bool,
    /// The shared Documents folder.
    documents: PathBuf,
}

/// Runs a `SystemPlugin.kt` command.
fn run<T: serde::de::DeserializeOwned>(app: &AppHandle, command: &str) -> Result<T, String> {
    app.state::<System>().0.run_mobile_plugin(command, ()).map_err(|e| e.to_string())
}

fn status(app: &AppHandle) -> Result<Status, String> {
    run(app, "status")
}

/// Sends the app to the background (Back on the library screen).
pub fn leave(app: &AppHandle) -> Result<(), String> {
    run::<serde_json::Value>(app, "moveTaskToBack").map(|_| ())
}

/// What the first-run screen offers: `Documents/Books`.
pub fn suggested_library(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(status(app)?.documents.join(LIBRARY_FOLDER))
}

/// `Documents/Books`, once All files access is on; until then this opens
/// Android's setting for it and asks the reader to come back. Picking another
/// folder needs a folder browser of our own (the full Android version).
pub fn chosen_library(window: &Window, pick: bool) -> Result<Option<PathBuf>, String> {
    if pick {
        return Err("Choosing another folder is not available on Android yet".into());
    }
    let app = window.app_handle();
    let status = status(app)?;
    if !status.granted {
        run::<serde_json::Value>(app, "askAllFilesAccess")?;
        return Err("Turn on \"Allow access to manage all files\" for Book Reader, come back and tap again.".into());
    }
    Ok(Some(status.documents.join(LIBRARY_FOLDER)))
}

/// Catalog passwords on Android (the Keystore) come with the full Android version.
pub fn save_password(_source_id: &str, _password: &str) -> Result<(), String> {
    Err("Saving catalog passwords on Android is not available yet".into())
}

/// No catalog passwords are kept on Android yet.
pub fn password(_source_id: &str) -> Option<String> {
    None
}

/// Nothing to forget while none are kept.
pub fn forget_password(_source_id: &str) {}

/// Fails while All files access is off (turned off after the library was set
/// up), saying where to turn it back on.
pub fn check_library_access(app: &AppHandle) -> Result<(), String> {
    if status(app)?.granted {
        return Ok(());
    }
    Err("Book Reader can't read the library: turn on \"Allow access to manage all files\" for it in Android's settings.".into())
}
