//! What the app needs from the OS: the contract both platforms implement.
//! `Active` is the one running now (`Desktop` on Windows, `Android` on a phone),
//! so adding a method here fails to compile until both sides implement it.

use crate::system_cmds::Outside;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Builder, Window, Wry};

pub(crate) trait Platform {
    /// The platform's Tauri plugins: file dialogs on the desktop, `SystemPlugin.kt` on a phone.
    fn plugins(builder: Builder<Wry>) -> Builder<Wry>;

    /// The library folder offered on first run.
    fn suggested_library(app: &AppHandle) -> Result<PathBuf, String>;

    /// The suggested folder, or one the reader picks (`None` when cancelled).
    /// On a phone this needs All files access, and may open Android's setting instead.
    fn chosen_library(window: &Window, pick: bool) -> Result<Option<PathBuf>, String>;

    /// Fails when the app cannot read its library folder (a phone with All
    /// files access off); nothing to check on the desktop.
    fn check_library_access(app: &AppHandle) -> Result<(), String>;

    /// A book file the reader picks (`None` when cancelled); the dialog
    /// offers only `extensions` (without dots), so disabled formats stay out.
    fn pick_book(window: &Window, extensions: &[&str]) -> Result<Option<Outside>, String>;

    /// A book handed to the app from outside ("Open with", "Share" on a
    /// phone); nothing waits on the desktop.
    fn take_incoming() -> Result<Option<Outside>, String>;

    /// Deletes the original of a book moved into the library; false where
    /// that isn't allowed, and always false on the desktop.
    fn delete_original(original: &str) -> bool;

    /// Deletes a book file from the library: to the Recycle Bin on the
    /// desktop, for good on a phone (which asked first).
    fn delete_book(path: &Path) -> Result<(), String>;

    /// A book is open (`on`) or closed: on a phone the screen stays on, the
    /// status bar hides, and the volume keys turn pages if `volume_keys` (the
    /// setting) is on. Nothing to do on the desktop.
    fn set_reading(on: bool, volume_keys: bool) -> Result<(), String>;

    /// Keeps a catalog's password: the OS credential store on the desktop,
    /// encrypted with a Keystore key on a phone.
    fn save_password(source_id: &str, password: &str) -> Result<(), String>;

    /// Whether a catalog has a password kept (without decrypting it).
    fn has_password(source_id: &str) -> bool;

    /// Whether each catalog has a password kept, in one call: one bridge
    /// round-trip on a phone, whatever the source count.
    fn has_passwords(source_ids: &[String]) -> Vec<bool>;

    /// A catalog's password, if one is kept.
    fn password(source_id: &str) -> Option<String>;

    /// Removes a catalog's password.
    fn forget_password(source_id: &str);

    /// Sends the app to the background (Back on the library screen of a
    /// phone); nothing to leave on the desktop.
    fn leave(app: &AppHandle) -> Result<(), String>;
}

#[cfg(desktop)]
#[path = "platform_desktop.rs"]
mod imp;
#[cfg(mobile)]
#[path = "platform_android.rs"]
mod imp;

#[cfg(desktop)]
pub(crate) use imp::Desktop as Active;
#[cfg(mobile)]
pub(crate) use imp::Android as Active;
