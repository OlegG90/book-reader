//! What the app needs from Android (see platform_desktop.rs for Windows),
//! through `SystemPlugin.kt`:
//! - the library: `Documents/Books` in shared storage (or a folder from
//!   Android's folder picker), reached with All files access. With the access
//!   on, the library is a plain folder and the rest of the app (scanning,
//!   Organize) works on it as on Windows;
//! - books from outside the library ("Open with", "Share", the book picker),
//!   copied into the app's own storage and opened from there;
//! - catalog passwords, encrypted with a key in the Android Keystore;
//! - reading: screen on, status bar hidden, volume keys.

use crate::system_cmds::Outside;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::plugin::{self, PluginHandle};
use tauri::{AppHandle, Builder, Window, Wry};

/// The library folder offered on first run, inside the shared Documents folder.
const LIBRARY_FOLDER: &str = "Books";

/// `SystemPlugin.kt`, registered once at startup.
static SYSTEM: OnceLock<PluginHandle<Wry>> = OnceLock::new();

/// Registers `SystemPlugin.kt`.
pub fn plugins(builder: Builder<Wry>) -> Builder<Wry> {
    builder.plugin(
        plugin::Builder::<Wry, ()>::new("system")
            .setup(|_app, api| {
                let _ = SYSTEM.set(api.register_android_plugin("com.olegg90.bookreader", "SystemPlugin")?);
                Ok(())
            })
            .build(),
    )
}

/// Runs a `SystemPlugin.kt` command.
fn run<T: DeserializeOwned>(command: &str, args: impl Serialize) -> Result<T, String> {
    SYSTEM.get().ok_or("The Android plugin isn't ready")?.run_mobile_plugin(command, args).map_err(|e| e.to_string())
}

/// Runs a `SystemPlugin.kt` command that answers nothing.
fn call(command: &str, args: impl Serialize) -> Result<(), String> {
    run::<Value>(command, args).map(|_| ())
}

/// What `SystemPlugin.status` reports.
#[derive(Deserialize)]
struct Status {
    /// All files access is on.
    granted: bool,
    /// The shared Documents folder.
    documents: PathBuf,
}

fn status() -> Result<Status, String> {
    run("status", ())
}

/// Sends the app to the background (Back on the library screen).
pub fn leave(_app: &AppHandle) -> Result<(), String> {
    call("moveTaskToBack", ())
}

/// What the first-run screen offers: `Documents/Books`.
pub fn suggested_library(_app: &AppHandle) -> Result<PathBuf, String> {
    Ok(status()?.documents.join(LIBRARY_FOLDER))
}

/// `Documents/Books`, or a folder from Android's folder picker (`None` when
/// cancelled), once All files access is on; until then this opens Android's
/// setting for it and asks the reader to come back.
pub fn chosen_library(_window: &Window, pick: bool) -> Result<Option<PathBuf>, String> {
    let status = status()?;
    if !status.granted {
        call("askAllFilesAccess", ())?;
        return Err("Turn on \"Allow access to manage all files\" for Book Reader, come back and tap again.".into());
    }
    if !pick {
        return Ok(Some(status.documents.join(LIBRARY_FOLDER)));
    }
    #[derive(Deserialize)]
    struct Picked {
        path: Option<PathBuf>,
    }
    Ok(run::<Picked>("pickFolder", ())?.path)
}

/// Fails while All files access is off (turned off after the library was set
/// up), saying where to turn it back on.
pub fn check_library_access(_app: &AppHandle) -> Result<(), String> {
    if status()?.granted {
        return Ok(());
    }
    Err("Book Reader can't read the library: turn on \"Allow access to manage all files\" for it in Android's settings.".into())
}

// ---- Books from outside the library ----------------------------------------------

/// What the plugin answers: the copy and its `content://` source, or nothing.
#[derive(Deserialize)]
struct Copied {
    path: Option<PathBuf>,
    uri: Option<String>,
}

impl Copied {
    fn outside(self) -> Option<Outside> {
        Some(Outside { path: self.path?, original: self.uri })
    }
}

/// Android's document picker (`None` when cancelled).
pub fn pick_book(_window: &Window) -> Result<Option<Outside>, String> {
    Ok(run::<Copied>("pickBook", ())?.outside())
}

/// A book handed to the app ("Open with", "Share") that is waiting to open.
pub fn take_incoming() -> Result<Option<Outside>, String> {
    Ok(run::<Copied>("takeIncoming", ())?.outside())
}

/// Deletes the original of a book moved into the library; false where
/// Android doesn't allow it (the book is then copied, not moved).
pub fn delete_original(original: &str) -> bool {
    #[derive(Deserialize)]
    struct Deleted {
        deleted: bool,
    }
    run::<Deleted>("deleteDocument", json!({ "uri": original })).is_ok_and(|d| d.deleted)
}

/// Deletes a book file from the library (a phone has no Recycle Bin for it;
/// the app asked first).
pub fn delete_book(path: &std::path::Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

// ---- Reading ------------------------------------------------------------------------

/// A book is open (`on`) or closed: the screen stays on, the status bar
/// hides, and the volume keys turn pages if `volume_keys` (the setting) is on.
pub fn set_reading(on: bool, volume_keys: bool) -> Result<(), String> {
    call("reading", json!({ "on": on, "volumeKeys": volume_keys }))
}

// ---- Catalog passwords ------------------------------------------------------------

/// Keeps a catalog's password, encrypted with a key in the Android Keystore.
pub fn save_password(source_id: &str, password: &str) -> Result<(), String> {
    call("savePassword", json!({ "id": source_id, "password": password }))
}

/// A catalog's password, if one is kept.
pub fn password(source_id: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Kept {
        password: Option<String>,
    }
    run::<Kept>("password", json!({ "id": source_id })).ok()?.password
}

/// Whether a catalog has a password kept (without decrypting it).
pub fn has_password(source_id: &str) -> bool {
    #[derive(Deserialize)]
    struct Kept {
        kept: bool,
    }
    run::<Kept>("hasPassword", json!({ "id": source_id })).is_ok_and(|k| k.kept)
}

/// Removes a catalog's password.
pub fn forget_password(source_id: &str) {
    let _ = call("forgetPassword", json!({ "id": source_id }));
}
