//! OPDS catalogs: sources, feeds, search, covers, downloads.

use crate::catalog;
use crate::clock;
use crate::fingerprint;
use crate::formats;
use crate::library_cmds::{library_root, put_in_library, record_book_source, AddMode, Added, LibraryBooks, CACHE_DIR};
use crate::modules;
use crate::opds;
use crate::platform::{Active, Platform};
use crate::store::Store;
use crate::CmdResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State, ipc::Response};

/// Search URL templates by source id, read once from each catalog.
#[derive(Default)]
pub(crate) struct SearchTemplates(Mutex<HashMap<String, String>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceView {
    #[serde(flatten)]
    source: catalog::Source,
    has_password: bool,
}

#[tauri::command(async)]
pub(crate) fn catalog_sources(store: State<Store>) -> CmdResult<Vec<SourceView>> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    let sources = store.read(|s| s.sources.clone());
    // One bridge round-trip for all sources, not one per source.
    let ids: Vec<_> = sources.iter().map(|s| s.id.clone()).collect();
    let kept = Active::has_passwords(&ids);
    Ok(sources.into_iter().zip(kept).map(|(source, has_password)| SourceView { has_password, source }).collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceInput {
    /// Missing for a new source.
    id: Option<String>,
    name: String,
    url: String,
    user: String,
    /// A new password; missing or empty keeps the one kept already.
    password: Option<String>,
    sync: bool,
}

#[derive(Serialize)]
pub(crate) struct SavedSource {
    id: String,
    /// Said when the address is plain HTTP across the internet.
    warning: Option<String>,
}

/// Adds or changes a catalog source, after finding its OPDS catalog (which
/// also checks the login). One source at most syncs progress.
#[tauri::command(async)]
pub(crate) fn save_catalog_source(
    app: AppHandle,
    store: State<Store>,
    templates: State<SearchTemplates>,
    input: SourceInput,
) -> CmdResult<SavedSource> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    let url = catalog::check_url(&input.url)?;
    let id = input.id.filter(|id| !id.is_empty()).unwrap_or_else(|| format!("{:x}", clock::now_nanos()));
    let name = match input.name.trim() {
        "" => url.host_str().unwrap_or("Catalog").to_string(),
        name => name.to_string(),
    };
    let mut source = catalog::Source { id: id.clone(), name, url: url.to_string(), user: input.user.trim().to_string(), sync: input.sync };
    // The reader types the server's address; the catalog may be under /opds.
    // The new password is tried first and kept only if the server takes it.
    let password = input.password.filter(|p| !p.is_empty());
    let url = catalog::discover(&source, catalog::login_with(&source, password.as_deref()).as_ref())?;
    source.url = url.to_string();
    if let Some(password) = password {
        Active::save_password(&id, &password)?;
    }
    store
        .update(|s| {
            if source.sync {
                s.sources.iter_mut().for_each(|other| other.sync = false);
            }
            match s.sources.iter_mut().find(|other| other.id == source.id) {
                Some(existing) => *existing = source,
                None => s.sources.push(source),
            }
        })
        .map_err(|e| e.to_string())?;
    templates.0.lock().unwrap().remove(&id);
    let _ = app.emit("catalogs-changed", ());
    Ok(SavedSource { id, warning: catalog::insecure_warning(&url) })
}

#[tauri::command(async)]
pub(crate) fn delete_catalog_source(app: AppHandle, store: State<Store>, id: String) -> CmdResult<()> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    store.update(|s| s.sources.retain(|source| source.id != id)).map_err(|e| e.to_string())?;
    Active::forget_password(&id);
    let _ = app.emit("catalogs-changed", ());
    Ok(())
}

fn source_by_id(store: &Store, id: &str) -> CmdResult<catalog::Source> {
    store.read(|s| s.sources.iter().find(|source| source.id == id).cloned()).ok_or_else(|| "That catalog was removed".into())
}

/// A page of a catalog: its root, or `url` on its server.
#[tauri::command(async)]
pub(crate) fn catalog_feed(store: State<Store>, id: String, url: Option<String>) -> CmdResult<opds::Feed> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    catalog::feed(&source_by_id(&store, &id)?, url.as_deref())
}

#[tauri::command(async)]
pub(crate) fn catalog_search(store: State<Store>, templates: State<SearchTemplates>, id: String, query: String) -> CmdResult<opds::Feed> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    let source = source_by_id(&store, &id)?;
    let known = templates.0.lock().unwrap().get(&id).cloned();
    let template = match known {
        Some(template) => template,
        None => {
            let template = catalog::search_template(&source)?;
            templates.0.lock().unwrap().insert(id, template.clone());
            template
        }
    };
    catalog::search(&source, &template, &query)
}

#[tauri::command(async)]
pub(crate) fn catalog_image(store: State<Store>, id: String, url: String) -> CmdResult<Response> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    catalog::image(&source_by_id(&store, &id)?, &url).map(Response::new)
}

/// Downloads a book from a catalog into the library, under the naming
/// template; a book the library already holds isn't added twice.
#[tauri::command(async)]
pub(crate) fn catalog_download(
    app: AppHandle,
    store: State<Store>,
    books: State<LibraryBooks>,
    id: String,
    url: String,
    format: opds::FileFormat,
) -> CmdResult<Added> {
    store.read(|s| modules::require(s, "capability:catalogs"))?;
    let source = source_by_id(&store, &id)?;
    let reader = formats::by_module(format.module()).ok_or("Not a book file")?;
    store.read(|s| formats::check(s, reader))?;
    let downloads = store.dir().join(CACHE_DIR).join("downloads");
    let file = catalog::download(&source, &url, format, &downloads)?;
    let result = put_in_library(&store, &books, &file, AddMode::Move);
    // Left behind when the library already had the book, or on a failure.
    let _ = std::fs::remove_file(&file);
    let added = result?.added;
    // Remember where the book came from, for its details. Best-effort:
    // a good download must not fail over bookkeeping.
    if let Ok(path) = library_root(&store).map(|root| root.join(&added.path)) {
        if let Ok(fp) = fingerprint::fingerprint(&path) {
            let _ = record_book_source(&store, &fp, &source.name);
        }
    }
    let _ = app.emit("library-updated", ());
    Ok(added)
}
