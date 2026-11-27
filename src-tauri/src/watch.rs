//! Watches the library folder on Windows: a book copied in, removed or
//! renamed there (or in a subfolder) has the open windows rescan the
//! library, without waiting for the window to come to the front.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{
    FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_DIR_NAME,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use crate::store::Store;

/// How often the watcher looks whether the library folder changed (or came back).
const CHECK_ROOT: Duration = Duration::from_secs(2);
/// A copy is many writes: the windows hear of it once the folder is this long quiet...
const QUIET: Duration = Duration::from_secs(1);
/// ...or after this long, should the folder never go quiet (a sync client, say).
const SETTLE_AT_MOST: Duration = Duration::from_secs(10);

/// What came of waiting on the folder.
#[derive(Debug, PartialEq)]
enum Wait {
    Changed,
    Quiet,
    /// The watch broke (its folder was deleted or renamed): it has to be made anew.
    Broken,
}

/// Windows' change notification on one folder and its subfolders; closed when dropped.
struct Watch(HANDLE);

impl Watch {
    /// Starts watching `dir` for files and folders that come, go, are renamed or
    /// change size; `None` when the folder can't be watched (it is gone, say).
    fn new(dir: &Path) -> Option<Self> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let what = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
        // SAFETY: `wide` is a NUL-terminated path that outlives the call.
        let handle = unsafe { FindFirstChangeNotificationW(wide.as_ptr(), 1, what) };
        (handle != INVALID_HANDLE_VALUE && !handle.is_null()).then_some(Self(handle))
    }

    /// Waits up to `timeout` for a change, and readies the watch for the next one.
    fn wait_change(&self, timeout: Duration) -> Wait {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the handle is open for the watch's whole life.
        match unsafe { WaitForSingleObject(self.0, millis) } {
            WAIT_TIMEOUT => Wait::Quiet,
            // SAFETY: as above.
            WAIT_OBJECT_0 if unsafe { FindNextChangeNotification(self.0) } != 0 => Wait::Changed,
            _ => Wait::Broken,
        }
    }

    /// After a change, waits until the folder goes quiet (or `SETTLE_AT_MOST`
    /// passes); false when the watch broke meanwhile.
    fn settle(&self) -> bool {
        let start = Instant::now();
        loop {
            match self.wait_change(QUIET) {
                Wait::Quiet => return true,
                Wait::Broken => return false,
                Wait::Changed if start.elapsed() >= SETTLE_AT_MOST => return true,
                Wait::Changed => {}
            }
        }
    }
}

impl Drop for Watch {
    /// Stops watching.
    fn drop(&mut self) {
        // SAFETY: the handle came from FindFirstChangeNotificationW and is closed once.
        unsafe { FindCloseChangeNotification(self.0) };
    }
}

/// Starts the watcher for the app's life. It follows the library folder as the
/// reader changes it, and tells every window `library-updated` once a change
/// has settled; the windows showing the library rescan it then.
pub(crate) fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let mut watching: Option<(PathBuf, Watch)> = None;
        loop {
            let root = app.state::<Store>().read(|s| s.library.clone());
            if watching.as_ref().map(|(dir, _)| dir) != root.as_ref() {
                watching = root.and_then(|dir| Watch::new(&dir).map(|watch| (dir, watch)));
            }
            let Some((_, watch)) = &watching else {
                std::thread::sleep(CHECK_ROOT);
                continue;
            };
            let settled = match watch.wait_change(CHECK_ROOT) {
                Wait::Quiet => continue,
                Wait::Changed => watch.settle(),
                Wait::Broken => false,
            };
            if !settled {
                // Made anew on the next round, once the folder is back.
                watching = None;
                std::thread::sleep(CHECK_ROOT);
            }
            // Either way something changed: a book came, or the folder went.
            let _ = app.emit("library-updated", ());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Wait, Watch};
    use std::time::Duration;

    #[test]
    fn a_new_file_in_a_subfolder_is_a_change() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Author")).unwrap();
        let watch = Watch::new(dir.path()).unwrap();
        assert_eq!(watch.wait_change(Duration::from_millis(100)), Wait::Quiet, "nothing happened yet");
        std::fs::write(dir.path().join("Author").join("Book.epub"), b"bytes").unwrap();
        assert_eq!(watch.wait_change(Duration::from_secs(5)), Wait::Changed);
        assert!(watch.settle(), "and then the folder goes quiet");
    }

    #[test]
    fn a_missing_folder_cannot_be_watched() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Watch::new(&dir.path().join("gone")).is_none());
    }
}
