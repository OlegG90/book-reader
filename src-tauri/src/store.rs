use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Everything the app remembers between runs, kept in one JSON file.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    /// Owned by the frontend; the backend stores it as-is.
    pub settings: Map<String, Value>,
    /// Reading positions keyed by book fingerprint.
    pub positions: HashMap<String, Position>,
    pub window: Option<WindowGeometry>,
    /// Recently opened books, newest first.
    pub recent: Vec<RecentBook>,
    /// The library folder; `None` until the first-run choice is made.
    pub library: Option<PathBuf>,
    /// Fingerprints of books the reader never wants offered for the library.
    pub declined: BTreeSet<String>,
    /// Bookmarks keyed by book fingerprint; owned by the frontend like settings.
    pub bookmarks: HashMap<String, Value>,
    /// OPDS catalogs (their passwords are in the OS credential store).
    pub sources: Vec<crate::catalog::Source>,
    /// Which catalog a book came from: fingerprint to the source's name at
    /// download time, for the book details.
    pub book_sources: HashMap<String, String>,
    /// What catalogs said about books: fingerprint to the feed's summary at
    /// download time, shown when the file itself carries no annotation.
    pub book_blurbs: HashMap<String, String>,
    /// This install's id for progress sync, so its own positions on the
    /// server can be told from another device's.
    pub device_id: String,
    /// Sync work still owed to the transports (see sync_cmds::engine); kept
    /// here so it survives a restart.
    #[serde(deserialize_with = "lenient_outbox")]
    pub outbox: Vec<Pending>,
}

/// Reads the outbox, skipping entries this build doesn't know (a job kind
/// from a newer version), so they never cost the rest of the config.
fn lenient_outbox<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Pending>, D::Error> {
    let raw = Vec::<Value>::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// One piece of sync work the engine still owes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Job {
    /// Send a book's position through one transport.
    #[serde(rename_all = "camelCase")]
    Position { transport: String, fingerprint: String, document: String, cfi: String, fraction: f64 },
    /// Upload a library book's file to the clouds that lack it.
    Upload { fingerprint: String },
    /// Download a cloud book the library lacks.
    Download { fingerprint: String },
}

impl Job {
    /// Two jobs with the same key are the same work: a newer one replaces
    /// the older (a book's newest position per transport, one upload or
    /// download per book).
    pub fn same_work(&self, other: &Job) -> bool {
        match (self, other) {
            (Job::Position { transport: a, fingerprint: f, .. }, Job::Position { transport: b, fingerprint: g, .. }) => a == b && f == g,
            (Job::Upload { fingerprint: f }, Job::Upload { fingerprint: g }) => f == g,
            (Job::Download { fingerprint: f }, Job::Download { fingerprint: g }) => f == g,
            _ => false,
        }
    }
}

/// A job in the outbox with its retry state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub job: Job,
    /// Failed tries so far; sets how long the next one waits.
    #[serde(default)]
    pub attempts: u32,
    /// Not tried before this time (ms since the Unix epoch).
    #[serde(default)]
    pub not_before: u64,
}

/// How many books the start screen lists.
const RECENT_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentBook {
    pub path: PathBuf,
    pub title: String,
    pub author: String,
    /// Links the entry to its saved position (for "% read").
    pub fingerprint: String,
}

impl State {
    /// Puts a book at the top of the recent list, once (by fingerprint, like
    /// positions), keeping the list short.
    pub fn remember(&mut self, book: RecentBook) {
        self.recent.retain(|r| r.fingerprint != book.fingerprint);
        self.recent.insert(0, book);
        self.recent.truncate(RECENT_LIMIT);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub cfi: String,
    pub fraction: f64,
    /// When it was saved (ms since the Unix epoch), to compare with the
    /// server's; 0 for positions saved before sync existed.
    #[serde(default)]
    pub time: u64,
    /// The device that saved it, when it came from another one through a
    /// cloud; `None` when this device saved it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Writer>,
}

/// Another device that saved a position, so this one passes it on under
/// that device's name instead of claiming it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Writer {
    pub device: String,
    pub device_id: String,
}

/// Window placement in logical pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

pub struct Store {
    path: PathBuf,
    state: Mutex<State>,
}

impl Store {
    /// Loads the state file. A missing file gives an empty state; a corrupt
    /// one is moved aside to `<name>.bak` rather than silently overwritten.
    pub fn load(path: PathBuf) -> Self {
        let state = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| {
                let _ = fs::rename(&path, path.with_extension("json.bak"));
                State::default()
            }),
            Err(_) => State::default(),
        };
        Store { path, state: Mutex::new(state) }
    }

    /// The folder the state file lives in, where caches go too.
    pub fn dir(&self) -> PathBuf {
        self.path.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    pub fn read<R>(&self, f: impl FnOnce(&State) -> R) -> R {
        f(&self.state.lock().unwrap())
    }

    /// Applies a change and writes the whole state to disk.
    pub fn update(&self, f: impl FnOnce(&mut State)) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        f(&mut state);
        write_atomically(&self.path, &serde_json::to_vec_pretty(&*state)?)
    }
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn position(cfi: &str) -> Position {
        Position { cfi: cfi.into(), fraction: 0.5, time: 0, by: None }
    }

    #[test]
    fn missing_file_gives_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::load(dir.path().join("breader.json"));
        assert_eq!(store.read(State::clone), State::default());
    }

    #[test]
    fn updates_survive_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("breader.json");
        let store = Store::load(path.clone());
        store
            .update(|s| {
                s.positions.insert("book".into(), position("epubcfi(/6/4)"));
            })
            .unwrap();
        let reloaded = Store::load(path);
        assert_eq!(reloaded.read(|s| s.positions["book"].clone()), position("epubcfi(/6/4)"));
    }

    #[test]
    fn recent_list_is_newest_first_without_duplicates() {
        let book = |path: &str| RecentBook {
            path: PathBuf::from(path),
            title: path.into(),
            author: String::new(),
            fingerprint: path.into(),
        };
        let mut state = State::default();
        for i in 0..12 {
            state.remember(book(&format!("{i}.epub")));
        }
        state.remember(book("5.epub"));
        let paths: Vec<_> = state.recent.iter().map(|r| r.path.to_string_lossy().into_owned()).collect();
        assert_eq!(paths.len(), RECENT_LIMIT);
        assert_eq!(&paths[..3], ["5.epub", "11.epub", "10.epub"]);
        assert_eq!(paths.iter().filter(|p| *p == "5.epub").count(), 1);
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("breader.json");
        fs::write(&path, "{ not json").unwrap();
        let store = Store::load(path.clone());
        assert_eq!(store.read(State::clone), State::default());
        assert_eq!(fs::read_to_string(path.with_extension("json.bak")).unwrap(), "{ not json");
    }

    #[test]
    fn disabled_modules_survive_reload_unknown_ids_included() {
        // Unknown ids are refused on write, but tolerated on load: an old
        // or newer config still opens, like with every other setting.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("breader.json");
        fs::write(&path, r#"{"settings":{"disabledModules":["format:fb2","wizardry:fireball",42]}}"#).unwrap();
        let store = Store::load(path.clone());
        assert_eq!(
            store.read(|s| s.settings["disabledModules"].clone()),
            serde_json::json!(["format:fb2", "wizardry:fireball", 42])
        );
        let reloaded = Store::load(path);
        assert_eq!(reloaded.read(|s| s.settings["disabledModules"].clone()), serde_json::json!(["format:fb2", "wizardry:fireball", 42]));
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("breader.json");
        fs::write(&path, r#"{"settings":{"flow":"scrolled"},"future":1}"#).unwrap();
        let store = Store::load(path);
        assert_eq!(store.read(|s| s.settings["flow"].clone()), "scrolled");
    }
}
