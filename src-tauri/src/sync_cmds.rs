//! Reading progress sync (KOSync): what the server says about a book.

use crate::catalog;
use crate::clock::now_ms;
use crate::kosync;
use crate::store::{Position, Store};
use crate::windows::ReaderWindows;
use crate::CmdResult;
use serde::Serialize;
use tauri::{Manager, State, Window};

/// The source that syncs progress, if one is set.
fn sync_source(store: &Store) -> Option<catalog::Source> {
    store.read(|s| s.sources.iter().find(|source| source.sync).cloned())
}

/// This install's id for the sync server, made once.
fn device_id(store: &Store) -> CmdResult<String> {
    let id = store.read(|s| s.device_id.clone());
    if !id.is_empty() {
        return Ok(id);
    }
    let made = format!("{:016x}", now_ms() ^ u64::from(std::process::id()).rotate_left(32));
    // Checked again under the store's lock: a pull and a push may both get here.
    let mut id = String::new();
    store
        .update(|s| {
            if s.device_id.is_empty() {
                s.device_id = made;
            }
            id = s.device_id.clone();
        })
        .map_err(|e| e.to_string())?;
    Ok(id)
}

const DEVICE_NAME: &str = if cfg!(mobile) {
    "Book Reader on Android"
} else if cfg!(windows) {
    "Book Reader on Windows"
} else {
    "Book Reader"
};

/// The window's book's id on the sync server, when its progress syncs
/// (there is a sync source, and the window keeps positions).
#[tauri::command(async)]
pub(crate) fn sync_document(window: Window, store: State<Store>) -> CmdResult<Option<String>> {
    let launch = window.state::<ReaderWindows>().get(window.label());
    let (Some(path), false, Some(_)) = (launch.file, launch.no_save, sync_source(&store)) else { return Ok(None) };
    kosync::document_id(&path).map(Some).map_err(|e| e.to_string())
}

/// A newer position from another device, offered when a book opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncOffer {
    cfi: String,
    fraction: f64,
    device: String,
    /// When the other device was there (ms since the Unix epoch).
    time: u64,
}

/// What the sync server's position means for the book just opened.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncState {
    /// A newer position from another device, to offer.
    offer: Option<SyncOffer>,
    /// The position here is newer than the server's (a push that didn't get
    /// through, or reading while offline): send it.
    push: bool,
}

/// Asks the sync server about a book (`document` from `sync_document`; its
/// local position under `fingerprint`). A server position is offered only if
/// it comes from another device, differs from the one here and is newer.
#[tauri::command(async)]
pub(crate) fn sync_pull(store: State<Store>, document: String, fingerprint: String) -> CmdResult<SyncState> {
    let Some(source) = sync_source(&store) else { return Ok(SyncState::default()) };
    let remote = kosync::pull(&source, &document)?;
    let local = store.read(|s| s.positions.get(&fingerprint).cloned());
    Ok(compare_positions(local.as_ref(), remote, &device_id(&store)?))
}

/// The newer position wins: the server's is offered when it comes from
/// another device; the local one is pushed when the server lags behind it.
fn compare_positions(local: Option<&Position>, remote: Option<kosync::Progress>, own_device: &str) -> SyncState {
    let Some(remote) = remote else { return SyncState { offer: None, push: local.is_some() } };
    let time = remote.timestamp * 1000;
    let here = local.map_or(0, |here| here.time);
    if local.is_some_and(|here| here.cfi == remote.progress) {
        return SyncState::default();
    }
    if here >= time || remote.device_id == own_device {
        return SyncState { offer: None, push: local.is_some() && here > time };
    }
    SyncState { offer: Some(SyncOffer { cfi: remote.progress, fraction: remote.percentage, device: remote.device, time }), push: false }
}

/// Sends a book's position to the sync server.
#[tauri::command(async)]
pub(crate) fn sync_push(store: State<Store>, document: String, cfi: String, fraction: f64) -> CmdResult<()> {
    let Some(source) = sync_source(&store) else { return Ok(()) };
    let device_id = device_id(&store)?;
    kosync::push(&source, &kosync::Update { document: &document, progress: &cfi, percentage: fraction, device: DEVICE_NAME, device_id: &device_id })
}

#[cfg(test)]
mod tests {
    use super::compare_positions;
    use crate::kosync::Progress;
    use crate::store::Position;

    fn here(cfi: &str, time: u64) -> Position {
        Position { cfi: cfi.into(), fraction: 0.5, time }
    }

    fn server(cfi: &str, device_id: &str, timestamp: u64) -> Progress {
        Progress { progress: cfi.into(), percentage: 0.6, device: "phone".into(), device_id: device_id.into(), timestamp }
    }

    #[test]
    fn a_newer_position_from_another_device_is_offered() {
        let state = compare_positions(Some(&here("a", 1_000)), Some(server("b", "phone", 2)), "pc");
        assert_eq!(state.offer.map(|o| o.cfi).as_deref(), Some("b"));
        assert!(!state.push);
        let state = compare_positions(None, Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_some(), "a book not read here yet");
    }

    #[test]
    fn a_newer_local_position_is_pushed() {
        let state = compare_positions(Some(&here("a", 3_000)), Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_none() && state.push);
        let state = compare_positions(Some(&here("a", 1)), None, "pc");
        assert!(state.offer.is_none() && state.push, "the server has nothing yet");
    }

    #[test]
    fn the_same_place_or_this_devices_own_position_changes_nothing() {
        let state = compare_positions(Some(&here("b", 1_000)), Some(server("b", "phone", 2)), "pc");
        assert!(state.offer.is_none() && !state.push);
        let state = compare_positions(Some(&here("a", 1_000)), Some(server("b", "pc", 2)), "pc");
        assert!(state.offer.is_none() && !state.push);
    }
}
