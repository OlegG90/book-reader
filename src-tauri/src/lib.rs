// Android uses only `Launch`: it has no command line.
#[cfg_attr(mobile, allow(dead_code))]
mod catalog;
mod cli;
mod clock;
mod cloud;
#[cfg(desktop)]
#[cfg_attr(not(windows), path = "console_stdout.rs")]
mod console;
// Android keeps its data in the app's storage: only `resolve_state_file` is shared.
#[cfg_attr(mobile, allow(dead_code))]
mod data_dir;
mod dropbox;
mod fingerprint;
mod formats;
mod google;
mod kosync;
mod library;
mod metadata;
mod modules;
mod naming;
mod net;
mod oauth;
mod opds;
mod paths;
#[cfg_attr(not(windows), path = "register_unsupported.rs")]
mod register;
// Where the library lives: a folder of the reader's choice, or Documents/Books on Android.
// The trait both platforms implement; `Active` is the one running now.
mod platform;
mod store;
mod updates;
#[cfg(desktop)]
mod version;
mod xml;
// Command handlers, by domain: windows, books, the library, catalogs, sync, system.
mod books;
mod catalog_cmds;
mod library_cmds;
mod sync_cmds;
mod sync_error;
mod system_cmds;
mod windows;

/// The error every command reports: a sentence for the reader.
pub(crate) type CmdResult<T> = Result<T, String>;

#[cfg(desktop)]
use cli::Command;
#[cfg(mobile)]
use cli::Launch;
use platform::Platform;
use std::path::Path;
use store::Store;
use tauri::Manager;
#[cfg(desktop)]
use tauri::DragDropEvent;
use tauri::WindowEvent;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The desktop app takes a book and options on the command line.
    #[cfg(desktop)]
    let (launch, data_dir) = {
        let cwd = std::env::current_dir().unwrap_or_default();
        match cli::parse(std::env::args().skip(1), &cwd) {
            Command::Open { launch, data_dir } => (launch, data_dir),
            other => std::process::exit(run_command(other)),
        }
    };
    #[cfg(mobile)]
    let launch = Launch::default();

    let builder = tauri::Builder::default();
    // Must come first: a second launch hands its arguments to this process and exits.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
        if let Command::Open { launch, .. } = cli::parse(argv.into_iter().skip(1), Path::new(&cwd)) {
            let _ = windows::open_or_focus(app, launch);
        }
    }));
    platform::Active::plugins(builder)
        .plugin(tauri_plugin_opener::init())
        .manage(windows::ReaderWindows::default())
        .manage(library_cmds::LibraryBooks::default())
        .manage(catalog_cmds::SearchTemplates::default())
        .manage(system_cmds::Originals::default())
        .invoke_handler(tauri::generate_handler![
            books::current_book,
            books::read_book,
            books::save_position,
            books::get_settings,
            books::update_settings,
            modules::list_modules,
            books::remember_book,
            books::recent_books,
            books::open_recent,
            books::forget_recent,
            books::clear_recent,
            system_cmds::register_file_types,
            system_cmds::unregister_file_types,
            system_cmds::file_types_registered,
            system_cmds::dropbox_status,
            system_cmds::dropbox_connect,
            system_cmds::dropbox_disconnect,
            system_cmds::google_status,
            system_cmds::google_connect,
            system_cmds::google_connect_android,
            system_cmds::google_disconnect,
            system_cmds::google_finish,
            system_cmds::google_take_answer,
            system_cmds::app_version,
            system_cmds::open_repository,
            system_cmds::check_for_update,
            system_cmds::open_latest_release,
            system_cmds::open_incoming,
            library_cmds::delete_library_book,
            system_cmds::set_reading,
            system_cmds::pick_book,
            library_cmds::library_info,
            library_cmds::set_library,
            library_cmds::scan_library,
            library_cmds::library_cover,
            library_cmds::book_details,
            library_cmds::book_cloud,
            library_cmds::open_library_book,
            system_cmds::show_library,
            system_cmds::leave_app,
            catalog_cmds::catalog_sources,
            catalog_cmds::save_catalog_source,
            catalog_cmds::delete_catalog_source,
            catalog_cmds::catalog_feed,
            catalog_cmds::catalog_search,
            catalog_cmds::catalog_image,
            catalog_cmds::catalog_download,
            sync_cmds::sync_document,
            sync_cmds::sync_pull,
            sync_cmds::sync_missing_books,
            sync_cmds::sync_queue_position,
            sync_cmds::sync_queue_upload,
            sync_cmds::sync_queue_downloads,
            sync_cmds::sync_wake,
            library_cmds::add_to_library,
            books::decline_library,
            books::get_bookmarks,
            books::set_bookmarks,
            library_cmds::organize_preview,
            library_cmds::organize_apply,
        ])
        .on_window_event(|window, event| match event {
            #[cfg(desktop)]
            WindowEvent::CloseRequested { .. } => {
                let _ = windows::remember_geometry(window);
            }
            #[cfg(desktop)]
            WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                let _ = windows::remember_geometry_throttled(window);
            }
            WindowEvent::Destroyed => {
                window.state::<windows::ReaderWindows>().remove(window.label());
            }
            #[cfg(desktop)]
            WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                if let Some(path) = paths.first() {
                    windows::show_book(window, path.clone(), false);
                }
            }
            _ => {}
        })
        .setup(move |app| {
            // Loaded here, in the one running app: a second launch hands over
            // its file and exits before setup, so it never touches the data file.
            // On Android the app's private storage keeps the state.
            #[cfg(mobile)]
            let data_dir = Some(app.path().app_data_dir()?);
            app.manage(Store::load(data_dir::resolve_state_file(data_dir.as_deref())));
            sync_cmds::start_engine(app.handle());
            windows::open_window(app.handle(), launch)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Book Reader");
}

/// Runs a command that doesn't open a window; returns the exit code.
#[cfg(desktop)]
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
                register::extensions().join(", ")
            ),
        ),
        Command::Unregister => report(register::unregister(), "Book Reader is no longer registered for any file types."),
        Command::Open { .. } => unreachable!("opening is handled by run()"),
    }
}

#[cfg(desktop)]
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
