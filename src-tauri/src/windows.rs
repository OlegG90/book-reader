//! The app's windows: which book each one shows, and opening new ones.

use crate::cli::Launch;
use crate::paths;
use crate::store::Store;
#[cfg(desktop)]
use crate::store::WindowGeometry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
#[cfg(desktop)]
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, Window};

#[cfg(desktop)]
const DEFAULT_WIDTH: f64 = 1100.0;
#[cfg(desktop)]
const DEFAULT_HEIGHT: f64 = 800.0;

/// What each reader window shows (its book and `--no-save`), by window label.
/// The frontend never passes file paths: it can only read the book the
/// backend assigned to its window, so book content cannot make the app read
/// arbitrary files.
#[derive(Default)]
pub(crate) struct ReaderWindows(Mutex<HashMap<String, Shown>>);

/// One window's book, and whether it was asked to close: such a window stays
/// until it is destroyed, so its page can still save, but no longer counts as open.
#[derive(Default)]
struct Shown {
    launch: Launch,
    closing: bool,
}

impl ReaderWindows {
    pub(crate) fn get(&self, label: &str) -> Launch {
        self.0.lock().unwrap().get(label).map(|w| w.launch.clone()).unwrap_or_default()
    }

    pub(crate) fn path_of(&self, label: &str) -> Result<PathBuf, String> {
        self.get(label).file.ok_or_else(|| "No book is open".into())
    }

    pub(crate) fn insert(&self, label: &str, launch: Launch) {
        self.0.lock().unwrap().insert(label.to_string(), Shown { launch, closing: false });
    }

    pub(crate) fn remove(&self, label: &str) {
        self.0.lock().unwrap().remove(label);
    }

    /// The window was asked to close: it is on its way out, though its page still saves.
    #[cfg(desktop)]
    pub(crate) fn mark_closing(&self, label: &str) {
        if let Some(w) = self.0.lock().unwrap().get_mut(label) {
            w.closing = true;
        }
    }

    pub(crate) fn set_path(&self, label: &str, path: PathBuf) {
        self.0.lock().unwrap().entry(label.to_string()).or_default().launch.file = Some(path);
    }

    /// Takes the window's "from the beginning" request: it holds for one load.
    pub(crate) fn take_from_start(&self, label: &str) -> bool {
        self.0.lock().unwrap().get_mut(label).is_some_and(|w| std::mem::take(&mut w.launch.from_start))
    }

    /// The label of the first open window (closing ones are left out) whose book matches.
    fn find_open(&self, matches: impl Fn(&str, &Launch) -> bool) -> Option<String> {
        let windows = self.0.lock().unwrap();
        windows.iter().find(|(label, w)| !w.closing && matches(label, &w.launch)).map(|(label, _)| label.clone())
    }

    /// The open window, other than `except`, already showing `path`.
    pub(crate) fn showing(&self, path: &Path, except: Option<&str>) -> Option<String> {
        self.find_open(|label, w| Some(label) != except && w.file.as_deref().is_some_and(|file| paths::same_file(file, path)))
    }

    /// Whether a window other than `label` stays open: one that is closing too doesn't count.
    pub(crate) fn has_others(&self, label: &str) -> bool {
        self.find_open(|other, _| other != label).is_some()
    }

    /// The library window's label: the open window without a book.
    pub(crate) fn library_window(&self) -> Option<String> {
        self.find_open(|_, w| w.file.is_none())
    }

    /// Points windows showing a file that moved at its new place.
    pub(crate) fn repath(&self, from: &Path, to: &Path) {
        for w in self.0.lock().unwrap().values_mut() {
            if w.launch.file.as_deref().is_some_and(|file| paths::same_file(file, from)) {
                w.launch.file = Some(to.to_path_buf());
            }
        }
    }
}

/// Brings a window forward; false when it is gone.
pub(crate) fn focus_window(app: &AppHandle, label: Option<String>) -> bool {
    let Some(window) = label.and_then(|label| app.get_webview_window(&label)) else { return false };
    #[cfg(desktop)]
    let _ = window.unminimize();
    window.set_focus().is_ok()
}

/// Brings forward the window (other than `except`) already showing `path`;
/// false when there is none.
pub(crate) fn focus_showing(app: &AppHandle, path: &Path, except: Option<&str>) -> bool {
    focus_window(app, app.state::<ReaderWindows>().showing(path, except))
}

/// Shows `path` in `window` and tells its frontend to load it (`from_start`:
/// at the beginning, not the saved position), unless another window already
/// shows that book; then that window comes forward instead (and, from the
/// start, reads it again). The library window stays the library: the book gets
/// a window of its own. A phone has one window, so there the book opens in it.
pub(crate) fn show_book(window: &Window, path: PathBuf, from_start: bool) {
    if cfg!(mobile) {
        return show_here(window, Some(path), from_start);
    }
    let windows = window.state::<ReaderWindows>();
    let current = windows.get(window.label());
    if let Some(label) = windows.showing(&path, current.file.is_some().then(|| window.label())) {
        if from_start {
            let no_save = windows.get(&label).no_save;
            windows.insert(&label, Launch { file: Some(path), no_save, from_start });
            let _ = window.emit_to(label.as_str(), "book-changed", ());
        }
        focus_window(window.app_handle(), Some(label));
        return;
    }
    // A `--no-save` library window passes that on to the books it opens.
    let launch = Launch { file: Some(path), no_save: current.no_save, from_start };
    if current.file.is_none() {
        let _ = open_window(window.app_handle(), launch);
        return;
    }
    windows.insert(window.label(), launch);
    let _ = window.emit_to(window.label(), "book-changed", ());
}

/// Shows a book, or the library for `None`, in `window` itself. A phone has
/// one window, so everything opens there and Back returns to the library.
pub(crate) fn show_here(window: &Window, file: Option<PathBuf>, from_start: bool) {
    let windows = window.state::<ReaderWindows>();
    let no_save = windows.get(window.label()).no_save;
    windows.insert(window.label(), Launch { file, no_save, from_start });
    let _ = window.emit_to(window.label(), "book-changed", ());
}

/// Fails for a `--no-save` window, which keeps nothing.
pub(crate) fn saving_window(windows: &ReaderWindows, window: &Window) -> Result<PathBuf, String> {
    let launch = windows.get(window.label());
    if launch.no_save {
        return Err("This window keeps nothing (--no-save)".into());
    }
    launch.file.ok_or_else(|| "No book is open".into())
}

/// Opens a window for `launch`, or brings forward the one already showing its
/// file (or, without a file, the library window).
///
/// Every command that can get here must be `async`: a sync command runs on the
/// main thread, and building a window there deadlocks on Windows.
pub(crate) fn open_or_focus(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    let focused = match launch.file.as_deref() {
        Some(file) => focus_showing(app, file, None),
        None => focus_window(app, app.state::<ReaderWindows>().library_window()),
    };
    if focused {
        return Ok(());
    }
    open_window(app, launch)
}

/// Blocks the WebView context menu in every frame (the app has no editing),
/// except in edit fields where the menu stays useful.
const NO_CONTEXT_MENU_SCRIPT: &str =
    "document.addEventListener('contextmenu', e => { if (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable]')) return; e.preventDefault(); });";

/// foliate-js's paginator re-lays pages out from a ResizeObserver, and the
/// browser then reports "ResizeObserver loop completed with undelivered
/// notifications". It is harmless (the rest arrives next frame), so it is
/// dropped before any other listener (such as Vite's dev overlay) sees it.
/// It has to be an initialization script: listeners on `window` run in the
/// order they were added, and this runs before the page's own scripts.
const QUIET_RESIZE_OBSERVER_SCRIPT: &str =
    "addEventListener('error', e => { if (String(e.message).includes('ResizeObserver loop')) { e.preventDefault(); e.stopImmediatePropagation(); } });";

pub(crate) fn open_window(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    let label = format!("reader-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let first = app.webview_windows().is_empty();
    app.state::<ReaderWindows>().insert(&label, launch);

    let theme = app.state::<Store>().read(|s| s.settings.get("theme").cloned());
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::default())
        .initialization_script(theme_script(theme.as_ref().and_then(|t| t.as_str())))
        .initialization_script_for_all_frames(NO_CONTEXT_MENU_SCRIPT)
        .initialization_script(QUIET_RESIZE_OBSERVER_SCRIPT);
    #[cfg(desktop)]
    return place_on_desktop(app, builder, first);
    // A phone has one full-screen window.
    #[cfg(mobile)]
    {
        let _ = first;
        builder.build().map(|_| ())
    }
}

/// Sizes and places a desktop window: the first one where the last window
/// was (if that is still on a screen), later ones cascaded by Windows.
#[cfg(desktop)]
fn place_on_desktop(app: &AppHandle, builder: WebviewWindowBuilder<tauri::Wry, AppHandle>, first: bool) -> tauri::Result<()> {
    let geometry = app.state::<Store>().read(|s| s.window.clone());
    let mut builder = builder.title("Book Reader").min_inner_size(400.0, 300.0).visible(false);
    builder = match &geometry {
        Some(g) if first => builder.inner_size(g.width, g.height).position(g.x, g.y).maximized(g.maximized),
        // Windows places (and cascades) later windows itself.
        Some(g) => builder.inner_size(g.width, g.height),
        None => builder.inner_size(DEFAULT_WIDTH, DEFAULT_HEIGHT).center(),
    };
    let window = builder.build()?;
    if first && geometry.is_some() && !is_on_screen(&window)? {
        window.center()?;
    }
    window.show()
}

/// Sets the saved theme before the first paint so the window never flashes
/// the wrong colours; the frontend takes over once its settings load.
fn theme_script(theme: Option<&str>) -> String {
    let resolved = match theme {
        Some(t @ ("light" | "dark" | "sepia")) => format!("'{t}'"),
        _ => "matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'".into(),
    };
    // The script may run before <html> exists; then wait for it.
    format!(
        "(() => {{
            const apply = () => !!document.documentElement && !!(document.documentElement.dataset.theme = {resolved});
            if (!apply()) new MutationObserver((_, o) => apply() && o.disconnect()).observe(document, {{ childList: true }});
        }})();"
    )
}

/// A saved position can point at a monitor that is no longer connected.
#[cfg(desktop)]
fn is_on_screen(window: &tauri::WebviewWindow) -> tauri::Result<bool> {
    let pos = window.outer_position()?;
    Ok(window.monitor_from_point(pos.x.into(), pos.y.into())?.is_some())
}

/// Saves the geometry at most every few seconds while the window moves or
/// resizes (those events fire continuously); a close always saves.
#[cfg(desktop)]
pub(crate) fn remember_geometry_throttled(window: &Window) -> tauri::Result<()> {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    const EVERY: Duration = Duration::from_secs(2);
    let due = LAST.lock().unwrap().is_none_or(|at| at.elapsed() >= EVERY);
    if !due {
        return Ok(());
    }
    *LAST.lock().unwrap() = Some(Instant::now());
    remember_geometry(window)
}

#[cfg(desktop)]
pub(crate) fn remember_geometry(window: &Window) -> tauri::Result<()> {
    if window.is_fullscreen()? {
        return Ok(());
    }
    let maximized = window.is_maximized()?;
    let scale = window.scale_factor()?;
    let pos = window.outer_position()?.to_logical::<f64>(scale);
    let size = window.inner_size()?.to_logical::<f64>(scale);
    let store = window.state::<Store>();
    store.update(|s| {
        // A maximized window keeps the size it will restore to.
        s.window = match (&s.window, maximized) {
            (Some(prev), true) => Some(WindowGeometry { maximized, ..prev.clone() }),
            _ => Some(WindowGeometry { x: pos.x, y: pos.y, width: size.width, height: size.height, maximized }),
        };
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{theme_script, ReaderWindows};
    use std::path::{Path, PathBuf};

    #[test]
    fn theme_script_uses_known_themes() {
        assert!(theme_script(Some("sepia")).contains("dataset.theme = 'sepia'"));
    }

    #[test]
    fn theme_script_falls_back_to_the_system_theme() {
        for theme in [None, Some("system"), Some("'; alert(1); '")] {
            let script = theme_script(theme);
            assert!(script.contains("prefers-color-scheme"), "{script}");
            assert!(!script.contains("alert"), "{script}");
        }
    }

    #[test]
    fn has_others_sees_only_other_open_windows() {
        let windows = ReaderWindows::default();
        windows.insert("reader-1", Default::default());
        assert!(!windows.has_others("reader-1"));
        windows.insert("reader-2", Default::default());
        assert!(windows.has_others("reader-1"));
    }

    #[test]
    fn closing_windows_no_longer_count_as_open() {
        let windows = ReaderWindows::default();
        windows.insert("reader-1", Default::default());
        windows.insert("reader-2", Default::default());
        windows.mark_closing("reader-2");
        // Closing both at once: the last one still sends the positions owed.
        assert!(!windows.has_others("reader-1"));
        assert_eq!(windows.library_window().as_deref(), Some("reader-1"));
        windows.mark_closing("reader-1");
        assert_eq!(windows.library_window(), None);
        // The closing window still reads its book until it is destroyed.
        windows.set_path("reader-1", "a.epub".into());
        assert_eq!(windows.path_of("reader-1").unwrap(), PathBuf::from("a.epub"));
        assert_eq!(windows.showing(Path::new("a.epub"), None), None);
    }
}
