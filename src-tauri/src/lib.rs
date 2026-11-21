mod cli;
mod console;
mod data_dir;
mod fingerprint;
mod library;
mod metadata;
mod paths;
mod register;
mod store;
mod version;

use cli::{Command, Launch};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use store::{Position, RecentBook, Store, WindowGeometry};
use tauri::ipc::Response;
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent};
use tauri_plugin_dialog::DialogExt;

const DEFAULT_WIDTH: f64 = 1100.0;
const DEFAULT_HEIGHT: f64 = 800.0;

/// What each reader window shows (its book and `--no-save`), by window label.
/// The frontend never passes file paths: it can only read the book the
/// backend assigned to its window, so book content cannot make the app read
/// arbitrary files.
#[derive(Default)]
struct ReaderWindows(Mutex<HashMap<String, Launch>>);

impl ReaderWindows {
    fn get(&self, label: &str) -> Launch {
        self.0.lock().unwrap().get(label).cloned().unwrap_or_default()
    }

    fn path_of(&self, label: &str) -> Result<PathBuf, String> {
        self.get(label).file.ok_or_else(|| "No book is open".into())
    }

    fn insert(&self, label: &str, launch: Launch) {
        self.0.lock().unwrap().insert(label.to_string(), launch);
    }

    fn remove(&self, label: &str) {
        self.0.lock().unwrap().remove(label);
    }

    fn set_path(&self, label: &str, path: PathBuf) {
        self.0.lock().unwrap().entry(label.to_string()).or_default().file = Some(path);
    }

    /// The window, other than `except`, already showing `path`.
    fn showing(&self, path: &Path, except: Option<&str>) -> Option<String> {
        let windows = self.0.lock().unwrap();
        windows
            .iter()
            .filter(|(label, _)| Some(label.as_str()) != except)
            .find(|(_, w)| w.file.as_deref().is_some_and(|file| paths::same_file(file, path)))
            .map(|(label, _)| label.clone())
    }

    /// A window without a book: it shows the library.
    fn library(&self) -> Option<String> {
        let windows = self.0.lock().unwrap();
        windows.iter().find(|(_, w)| w.file.is_none()).map(|(label, _)| label.clone())
    }
}

/// Brings a window forward; false when it is gone.
fn focus_window(app: &AppHandle, label: Option<String>) -> bool {
    let Some(window) = label.and_then(|label| app.get_webview_window(&label)) else { return false };
    let _ = window.unminimize();
    window.set_focus().is_ok()
}

/// Brings forward the window (other than `except`) already showing `path`;
/// false when there is none.
fn focus_showing(app: &AppHandle, path: &Path, except: Option<&str>) -> bool {
    focus_window(app, app.state::<ReaderWindows>().showing(path, except))
}

/// Shows `path` in `window` and tells its frontend to load it, unless another
/// window already shows that book; then that window comes forward instead.
/// The library window stays the library: the book gets a window of its own.
fn show_book(window: &Window, path: PathBuf) {
    if window.state::<ReaderWindows>().get(window.label()).file.is_none() {
        let _ = open_or_focus(window.app_handle(), Launch { file: Some(path), no_save: false });
        return;
    }
    if focus_showing(window.app_handle(), &path, Some(window.label())) {
        return;
    }
    window.state::<ReaderWindows>().set_path(window.label(), path);
    let _ = window.emit_to(window.label(), "book-changed", ());
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BookInfo {
    file_name: String,
    fingerprint: String,
    position: Option<Position>,
    no_save: bool,
}

#[tauri::command(async)]
fn current_book(window: Window, windows: State<ReaderWindows>, store: State<Store>) -> Result<Option<BookInfo>, String> {
    let Launch { file: Some(path), no_save } = windows.get(window.label()) else {
        return Ok(None);
    };
    let file_name = paths::file_name(&path);
    let fingerprint = fingerprint::fingerprint(&path).map_err(|e| format!("{file_name}: {e}"))?;
    let position = store.read(|s| s.positions.get(&fingerprint).cloned());
    Ok(Some(BookInfo { file_name, fingerprint, position, no_save }))
}

#[tauri::command(async)]
fn read_book(window: Window, windows: State<ReaderWindows>) -> Result<Response, String> {
    let path = windows.path_of(window.label())?;
    std::fs::read(path).map(Response::new).map_err(|e| e.to_string())
}

/// Takes the fingerprint from the frontend so a position that is still
/// pending when another book replaces it is saved under the right book.
#[tauri::command(async)]
fn save_position(store: State<Store>, fingerprint: String, position: Position) -> Result<(), String> {
    store
        .update(|s| {
            s.positions.insert(fingerprint, position);
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(store: State<Store>) -> Map<String, Value> {
    store.read(|s| s.settings.clone())
}

/// Merges changed settings (so windows never overwrite each other's changes)
/// and passes them on to the other windows.
#[tauri::command(async)]
fn update_settings(window: Window, store: State<Store>, changes: Map<String, Value>) -> Result<(), String> {
    store.update(|s| s.settings.extend(changes.clone())).map_err(|e| e.to_string())?;
    for label in window.app_handle().webview_windows().into_keys().filter(|l| l != window.label()) {
        let _ = window.emit_to(&label, "settings-changed", &changes);
    }
    Ok(())
}

/// Adds the window's book to the recent list, with the title and author the
/// frontend read from it.
#[tauri::command(async)]
fn remember_book(
    window: Window,
    windows: State<ReaderWindows>,
    store: State<Store>,
    fingerprint: String,
    title: String,
    author: String,
) -> Result<(), String> {
    let Launch { file: Some(path), no_save: false } = windows.get(window.label()) else {
        return Ok(());
    };
    let book = RecentBook { path, title, author, fingerprint };
    // Reopening the latest book (or reloading it) changes nothing: skip the write.
    if store.read(|s| s.recent.first() == Some(&book)) {
        return Ok(());
    }
    store.update(|s| s.remember(book)).map_err(|e| e.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentView {
    /// Identifies the entry (the book's fingerprint), so a click still finds
    /// the right book after the list changed in another window.
    key: String,
    title: String,
    author: String,
    file_name: String,
    /// Share read, if a position is saved.
    fraction: Option<f64>,
    /// False when the file has been moved or deleted.
    exists: bool,
}

#[tauri::command(async)]
fn recent_books(store: State<Store>) -> Vec<RecentView> {
    let books = store.read(|s| {
        let fraction = |r: &RecentBook| s.positions.get(&r.fingerprint).map(|p| p.fraction);
        s.recent.iter().map(|r| (r.clone(), fraction(r))).collect::<Vec<_>>()
    });
    // Checked outside the store lock: a slow drive shouldn't hold up other windows.
    books
        .into_iter()
        .map(|(r, fraction)| RecentView {
            file_name: paths::file_name(&r.path),
            exists: r.path.is_file(),
            key: r.fingerprint,
            title: r.title,
            author: r.author,
            fraction,
        })
        .collect()
}

/// Opens a recent book in this window. Books are picked by their key, so the
/// frontend still never supplies a path.
#[tauri::command(async)]
fn open_recent(window: Window, store: State<Store>, key: String) -> Result<(), String> {
    let path = store
        .read(|s| s.recent.iter().find(|r| r.fingerprint == key).map(|r| r.path.clone()))
        .ok_or("That book is no longer in the recent list")?;
    show_book(&window, path);
    Ok(())
}

#[tauri::command(async)]
fn forget_recent(store: State<Store>, key: String) -> Result<(), String> {
    store.update(|s| s.recent.retain(|r| r.fingerprint != key)).map_err(|e| e.to_string())
}

/// Forgets the recent books list. Reading positions are kept, so reopened books resume.
#[tauri::command(async)]
fn clear_recent(store: State<Store>) -> Result<(), String> {
    store.update(|s| s.recent.clear()).map_err(|e| e.to_string())
}

/// Registers the app for its book file types (see register::register).
#[tauri::command(async)]
fn register_file_types() -> Result<(), String> {
    std::env::current_exe().and_then(|exe| register::register(&exe)).map_err(|e| e.to_string())
}

/// Removes the app's file associations again.
#[tauri::command(async)]
fn unregister_file_types() -> Result<(), String> {
    register::unregister().map_err(|e| e.to_string())
}

/// Whether the app is registered for its book file types.
#[tauri::command(async)]
fn file_types_registered() -> bool {
    register::registered()
}

/// The app version (from Cargo.toml), shown in the About panel.
#[tauri::command]
fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Ctrl+O: asks for a book and opens it in this window.
#[tauri::command(async)]
fn pick_book(window: Window) -> Result<(), String> {
    let picked = window
        .dialog()
        .file()
        .set_parent(&window)
        .add_filter("Books", &["epub", "fb2", "fb2.zip", "fbz"])
        .blocking_pick_file();
    if let Some(path) = picked {
        show_book(&window, path.into_path().map_err(|e| e.to_string())?);
    }
    Ok(())
}

/// Opens a window for `launch`, or brings forward the one already showing its
/// file (or, without a file, the library window).
fn open_or_focus(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    let focused = match launch.file.as_deref() {
        Some(file) => focus_showing(app, file, None),
        None => focus_window(app, app.state::<ReaderWindows>().library()),
    };
    if focused {
        return Ok(());
    }
    open_window(app, launch)
}

// ---- Library ------------------------------------------------------------------

/// The books found by the last scan. The frontend names books by their key
/// (the path inside the library), which is only accepted when the scan found
/// it, so the frontend still cannot make the app open arbitrary files.
#[derive(Default)]
struct LibraryBooks(Mutex<Vec<library::Entry>>);

impl LibraryBooks {
    fn path_of(&self, store: &Store, key: &str) -> Result<PathBuf, String> {
        let root = library_root(store)?;
        let books = self.0.lock().unwrap();
        let entry = books.iter().find(|b| b.path == key).ok_or("That book is no longer in the library")?;
        Ok(entry.absolute(&root))
    }
}

fn library_root(store: &Store) -> Result<PathBuf, String> {
    store.read(|s| s.library.clone()).ok_or_else(|| "No library folder is set".into())
}

fn library_cache(store: &Store) -> library::Cache {
    library::Cache::new(store.dir().join("breader-cache"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryInfo {
    /// The library folder, once chosen.
    folder: Option<String>,
    /// What the first-run screen offers.
    suggested: String,
}

#[tauri::command]
fn library_info(store: State<Store>) -> LibraryInfo {
    LibraryInfo {
        folder: store.read(|s| s.library.as_ref().map(|p| p.display().to_string())),
        suggested: data_dir::resolve_default_library().display().to_string(),
    }
}

/// Sets the library folder: the suggested one, or one the reader picks.
/// Returns the folder, or `None` when the picker was cancelled.
#[tauri::command(async)]
fn set_library(window: Window, store: State<Store>, pick: bool) -> Result<Option<String>, String> {
    let folder = if pick {
        match window.dialog().file().set_parent(&window).blocking_pick_folder() {
            Some(folder) => folder.into_path().map_err(|e| e.to_string())?,
            None => return Ok(None),
        }
    } else {
        data_dir::resolve_default_library()
    };
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let folder = std::path::absolute(&folder).unwrap_or(folder);
    store.update(|s| s.library = Some(folder.clone())).map_err(|e| e.to_string())?;
    let _ = window.emit("library-changed", ());
    Ok(Some(folder.display().to_string()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LibraryBook {
    key: String,
    title: String,
    /// "First Last", one per author.
    authors: Vec<String>,
    /// "Last First" of the first author, for sorting.
    author_sort: String,
    series: Option<String>,
    series_index: Option<String>,
    format: &'static str,
    /// Share read, if a position is saved.
    fraction: Option<f64>,
    /// When the file was last changed (ms), for "recently added".
    added: u64,
    has_cover: bool,
}

/// Scans the library folder (cheap after the first time: see library::scan).
#[tauri::command(async)]
fn scan_library(store: State<Store>, books: State<LibraryBooks>) -> Result<Vec<LibraryBook>, String> {
    let root = library_root(&store)?;
    let entries = library::scan(&root, &library_cache(&store)).map_err(|e| format!("{}: {e}", root.display()))?;
    let views = store.read(|s| {
        entries
            .iter()
            .map(|e| LibraryBook {
                key: e.path.clone(),
                title: e.title.clone(),
                authors: e.authors.iter().map(metadata::Author::display).collect(),
                author_sort: e.authors.first().map(metadata::Author::sort_name).unwrap_or_default(),
                series: e.series.clone(),
                series_index: e.series_index.clone(),
                format: match metadata::Format::of(Path::new(&e.path)) {
                    Some(metadata::Format::Epub) => "EPUB",
                    _ => "FB2",
                },
                fraction: s.positions.get(&e.fingerprint).map(|p| p.fraction),
                added: e.modified,
                has_cover: e.cover.is_some(),
            })
            .collect()
    });
    *books.0.lock().unwrap() = entries;
    Ok(views)
}

#[tauri::command(async)]
fn library_cover(store: State<Store>, books: State<LibraryBooks>, key: String) -> Result<Response, String> {
    let cover = books.0.lock().unwrap().iter().find(|b| b.path == key).and_then(|b| b.cover.clone()).ok_or("No cover")?;
    std::fs::read(library_cache(&store).covers().join(cover)).map(Response::new).map_err(|e| e.to_string())
}

/// Opens a library book in a window of its own.
#[tauri::command(async)]
fn open_library_book(app: AppHandle, store: State<Store>, books: State<LibraryBooks>, key: String) -> Result<(), String> {
    let path = books.path_of(&store, &key)?;
    open_or_focus(&app, Launch { file: Some(path), no_save: false }).map_err(|e| e.to_string())
}

/// Brings the library window forward, opening it if needed.
#[tauri::command]
fn show_library(app: AppHandle) -> Result<(), String> {
    open_or_focus(&app, Launch::default()).map_err(|e| e.to_string())
}

/// Blocks the WebView context menu in every frame (the app has no editing),
/// except in edit fields where the menu stays useful.
const NO_CONTEXT_MENU_SCRIPT: &str =
    "document.addEventListener('contextmenu', e => { if (e.target instanceof Element && e.target.closest('input, textarea, select, [contenteditable]')) return; e.preventDefault(); });";

fn open_window(app: &AppHandle, launch: Launch) -> tauri::Result<()> {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    let label = format!("reader-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
    let first = app.webview_windows().is_empty();
    app.state::<ReaderWindows>().insert(&label, launch);

    let (geometry, theme) = app.state::<Store>().read(|s| (s.window.clone(), s.settings.get("theme").cloned()));
    let mut builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::default())
        .title("Book Reader")
        .min_inner_size(400.0, 300.0)
        .initialization_script(theme_script(theme.as_ref().and_then(|t| t.as_str())))
        .initialization_script_for_all_frames(NO_CONTEXT_MENU_SCRIPT)
        .visible(false);
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
fn is_on_screen(window: &tauri::WebviewWindow) -> tauri::Result<bool> {
    let pos = window.outer_position()?;
    Ok(window.monitor_from_point(pos.x.into(), pos.y.into())?.is_some())
}

fn remember_geometry(window: &Window) -> tauri::Result<()> {
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

pub fn run() {
    let cwd = std::env::current_dir().unwrap_or_default();
    let (launch, data_dir) = match cli::parse(std::env::args().skip(1), &cwd) {
        Command::Open { launch, data_dir } => (launch, data_dir),
        other => std::process::exit(run_command(other)),
    };

    tauri::Builder::default()
        // Must come first: a second launch hands its arguments to this process and exits.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            if let Command::Open { launch, .. } = cli::parse(argv.into_iter().skip(1), Path::new(&cwd)) {
                let _ = open_or_focus(app, launch);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(ReaderWindows::default())
        .manage(LibraryBooks::default())
        .invoke_handler(tauri::generate_handler![
            current_book,
            read_book,
            save_position,
            get_settings,
            update_settings,
            remember_book,
            recent_books,
            open_recent,
            forget_recent,
            clear_recent,
            register_file_types,
            unregister_file_types,
            file_types_registered,
            app_version,
            pick_book,
            library_info,
            set_library,
            scan_library,
            library_cover,
            open_library_book,
            show_library,
        ])
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { .. } => {
                let _ = remember_geometry(window);
            }
            WindowEvent::Destroyed => {
                window.state::<ReaderWindows>().remove(window.label());
            }
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                if let Some(path) = paths.first() {
                    show_book(window, path.clone());
                }
            }
            _ => {}
        })
        .setup(move |app| {
            // Loaded here, in the one running app: a second launch hands over
            // its file and exits before setup, so it never touches the data file.
            app.manage(Store::load(data_dir::resolve_state_file(data_dir.as_deref())));
            open_window(app.handle(), launch)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Book Reader");
}

/// Runs a command that doesn't open a window; returns the exit code.
fn run_command(command: Command) -> i32 {
    match command {
        Command::Help => report(Ok(()), cli::USAGE),
        Command::Version => report(Ok(()), &version::text(tauri::webview_version().ok().as_deref())),
        Command::Invalid(message) => {
            console::print(&format!("{message}\n\n{}", cli::USAGE));
            2
        }
        Command::Register => report(
            std::env::current_exe().and_then(|exe| register::register(&exe)),
            &format!(
                "Book Reader is registered for {}.\nIf another app still opens them, right-click a file > Open with > \
                 Choose another app > Book Reader, and tick \"Always\".",
                register::EXTENSIONS.join(", ")
            ),
        ),
        Command::Unregister => report(register::unregister(), "Book Reader is no longer registered for any file types."),
        Command::Open { .. } => unreachable!("opening is handled by run()"),
    }
}

fn report(result: std::io::Result<()>, success: &str) -> i32 {
    match result {
        Ok(()) => {
            console::print(success);
            0
        }
        Err(e) => {
            console::print(&format!("Failed: {e}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::theme_script;

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
}
