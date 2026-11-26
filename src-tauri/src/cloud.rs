//! What the cloud transports (Dropbox, Google Drive) share. Each cloud is a
//! `CloudStore`: a flat folder of named files behind an access token. On top
//! of that, written once here: the fingerprint-keyed positions file and its
//! newest-wins merge, the book manifest and book files, the access token
//! kept until it expires, and a slot for the last failure each cloud shows
//! in the settings.

use crate::kosync;
use crate::platform::{Active, Platform};
use crate::store::Position;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One cloud as the sync code sees it: named files in the app's own folder,
/// read and written with an access token. Each cloud only knows its HTTP;
/// what lives in the files is the same everywhere (see the functions below).
pub trait CloudStore: Sync {
    /// The transport id ("dropbox", "google") that the frontend's pending set names.
    fn id(&self) -> &'static str;
    /// The Modules switch that turns the cloud off.
    fn module(&self) -> &'static str;
    /// Where the refresh token is kept in the credential store.
    fn token_id(&self) -> &'static str;
    /// The cloud's name in messages ("Dropbox", "Google Drive").
    fn label(&self) -> &'static str;
    /// Trades the refresh token for an access token and how many seconds it lives.
    fn refresh(&self, refresh_token: &str) -> Result<(String, u64), String>;
    /// A file's bytes by name; `None` when the cloud has no such file.
    fn get(&self, access: &str, name: &str) -> Result<Option<Vec<u8>>, String>;
    /// Writes a file by name, creating it or replacing what is there.
    fn put(&self, access: &str, name: &str, bytes: &[u8]) -> Result<(), String>;
    /// The cloud's last failure, for the settings.
    fn errors(&self) -> &ErrorSlot;
    /// The access token while it lives, so one sync round asks for it once.
    fn tokens(&self) -> &TokenCache;

    /// Whether a refresh token is kept (a login happened).
    fn connected(&self) -> bool {
        Active::has_password(self.token_id())
    }

    /// An access token: the kept one while it lives, else a fresh one.
    fn access_token(&self) -> Result<String, String> {
        if let Some(token) = self.tokens().fresh() {
            return Ok(token);
        }
        let refresh = Active::password(self.token_id()).ok_or_else(|| format!("{} is not connected.", self.label()))?;
        let (token, lives) = self.refresh(&refresh)?;
        self.tokens().keep(&token, lives);
        Ok(token)
    }
}

/// An access token kept until shortly before it expires.
pub struct TokenCache(Mutex<Option<(String, Instant)>>);

impl TokenCache {
    /// An empty cache, for the cloud's static.
    pub const fn new() -> Self {
        TokenCache(Mutex::new(None))
    }

    /// The kept token, unless it is about to expire.
    fn fresh(&self) -> Option<String> {
        let kept = self.0.lock().unwrap();
        kept.as_ref().filter(|(_, until)| Instant::now() < *until).map(|(token, _)| token.clone())
    }

    /// Keeps a token for `lives` seconds, minus a minute of slack.
    fn keep(&self, token: &str, lives: u64) {
        let until = Instant::now() + Duration::from_secs(lives.saturating_sub(60));
        *self.0.lock().unwrap() = Some((token.into(), until));
    }

    /// Forgets the token (a new login, or a disconnect).
    pub fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

/// The positions file in every cloud's app folder.
const POSITIONS_FILE: &str = "sync-state.json";
/// The positions file's envelope version.
const POSITIONS_VERSION: u32 = 1;
/// The book manifest in every cloud's app folder.
const MANIFEST_FILE: &str = "sync-books.json";

/// Every cloud, in merge order.
pub fn stores() -> [&'static dyn CloudStore; 2] {
    [&crate::dropbox::STORE, &crate::google::STORE]
}

/// Forgets a cloud's login: the refresh token and the access token.
pub fn disconnect(store: &dyn CloudStore) {
    Active::forget_password(store.token_id());
    store.tokens().clear();
}

/// Keeps the refresh token of a new login; an access token of an earlier
/// account is dropped with it.
pub fn keep_login(store: &dyn CloudStore, refresh: &str) -> Result<(), String> {
    store.tokens().clear();
    Active::save_password(store.token_id(), refresh)
}

/// The positions file: `None` before the first sync, an error when it cannot be read.
fn positions_file(store: &dyn CloudStore, access: &str) -> Result<Option<HashMap<String, RemoteRecord>>, String> {
    let Some(body) = store.get(access, POSITIONS_FILE)? else { return Ok(None) };
    let file: SyncFile = serde_json::from_slice(&body).map_err(|e| format!("{} file is not ours: {e}", store.label()))?;
    Ok(Some(file.positions))
}

/// The cloud's whole positions file, if the app ever synced; a dead cloud
/// reports nothing (its failure goes to the settings).
pub fn remote_positions(store: &dyn CloudStore) -> Option<HashMap<String, RemoteRecord>> {
    store.errors().track(store.access_token().and_then(|access| positions_file(store, &access))).ok().flatten()
}

/// The open book's position in the cloud, in KOSync's shape (time in
/// seconds) so it merges with the other transports' answers; a dead cloud
/// reports nothing instead of failing the pull.
pub fn pull(store: &dyn CloudStore, fingerprint: &str) -> Option<kosync::Progress> {
    remote_positions(store)?.remove(fingerprint).map(|r| kosync::Progress {
        progress: r.cfi,
        percentage: r.fraction,
        device: r.device,
        device_id: r.device_id,
        timestamp: r.time / 1000,
    })
}

/// Adds the open book's new position to the local ones, merges them over
/// the cloud file (newest per book wins) and uploads it when anything changed.
pub fn push(
    store: &dyn CloudStore,
    local: &HashMap<String, Position>,
    fingerprint: &str,
    place: (&str, f64),
    device: &str,
    device_id: &str,
) -> Result<(), String> {
    let pushed = (|| {
        let access = store.access_token()?;
        let remote = positions_file(store, &access)?.unwrap_or_default();
        let mut local = local.clone();
        local.insert(fingerprint.into(), Position { cfi: place.0.into(), fraction: place.1, time: crate::clock::now_ms() });
        let merged = merge_for_upload(&local, &remote, device, device_id);
        if !should_upload(&merged, &remote) {
            return Ok(());
        }
        let body = serde_json::to_vec(&SyncFile { version: POSITIONS_VERSION, positions: merged }).map_err(|e| e.to_string())?;
        store.put(&access, POSITIONS_FILE, &body)
    })();
    store.errors().track(pushed)
}

/// The cloud's book manifest: `None` when no book was ever uploaded, an
/// error when it could not be read (never mistaken for an empty one).
pub fn manifest(store: &dyn CloudStore) -> Result<Option<HashMap<String, BookRecord>>, String> {
    let access = store.access_token()?;
    let Some(body) = store.get(&access, MANIFEST_FILE)? else { return Ok(None) };
    parse_manifest(&body).map(Some)
}

/// The cloud's book manifest, if the app ever synced books; a failure goes
/// to the settings.
pub fn book_manifest(store: &dyn CloudStore) -> Option<HashMap<String, BookRecord>> {
    store.errors().track(manifest(store)).ok().flatten()
}

/// A book file by its cloud name; `None` when the cloud never saw it.
pub fn download_book(store: &dyn CloudStore, remote_name: &str) -> Result<Option<Vec<u8>>, String> {
    store.errors().track(store.access_token().and_then(|access| store.get(&access, remote_name)))
}

/// Uploads a book file the cloud lacks and records it in the manifest; true
/// when the cloud gained the book.
pub fn push_book(store: &dyn CloudStore, fingerprint: &str, record: &BookRecord, bytes: &[u8]) -> Result<bool, String> {
    let pushed = store.access_token().and_then(|access| {
        push_book_missing(
            fingerprint,
            record,
            bytes,
            || manifest(store),
            |name, bytes| store.put(&access, name, bytes),
            |books| store.put(&access, MANIFEST_FILE, &render_manifest(books)?),
        )
    });
    store.errors().track(pushed)
}

/// One book's synced position as a cloud file keeps it: the local shape
/// (see store::Position) plus who saved it, for the echo check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRecord {
    pub cfi: String,
    pub fraction: f64,
    /// ms since the Unix epoch, like local positions.
    pub time: u64,
    pub device: String,
    pub device_id: String,
}

/// The positions file envelope: the version plus the books.
/// Unknown fields are tolerated on load, for forward compatibility.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SyncFile {
    pub version: u32,
    #[serde(default)]
    pub positions: HashMap<String, RemoteRecord>,
}

/// Merges remote records into the local positions for the upload: newest per
/// book wins; ties and local-only books keep the local position, stamped
/// with this install (the echo check needs a writer).
pub fn merge_for_upload(
    local: &HashMap<String, crate::store::Position>,
    remote: &HashMap<String, RemoteRecord>,
    device: &str,
    device_id: &str,
) -> HashMap<String, RemoteRecord> {
    let mut merged = remote.clone();
    for (fingerprint, pos) in local {
        let newer_remote = remote.get(fingerprint).is_some_and(|r| r.time > pos.time);
        if !newer_remote {
            merged.insert(
                fingerprint.clone(),
                RemoteRecord {
                    cfi: pos.cfi.clone(),
                    fraction: pos.fraction,
                    time: pos.time,
                    device: device.into(),
                    device_id: device_id.into(),
                },
            );
        }
    }
    merged
}

/// Whether the merged file differs from what the cloud has.
pub fn should_upload(merged: &HashMap<String, RemoteRecord>, remote: &HashMap<String, RemoteRecord>) -> bool {
    merged != remote
}

/// One book file the cloud keeps, keyed by fingerprint in `BookManifest`.
/// Same fingerprint means same bytes, so records never conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookRecord {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// The fingerprint-keyed book manifest envelope; unknown fields are
/// tolerated on load, for forward compatibility.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BookManifest {
    pub version: u32,
    #[serde(default)]
    pub books: HashMap<String, BookRecord>,
}

/// Parses a cloud book manifest.
pub fn parse_manifest(bytes: &[u8]) -> Result<HashMap<String, BookRecord>, String> {
    let manifest: BookManifest =
        serde_json::from_slice(bytes).map_err(|e| format!("book manifest is not ours: {e}"))?;
    Ok(manifest.books)
}

/// Renders a cloud book manifest.
pub fn render_manifest(books: &HashMap<String, BookRecord>) -> Result<Vec<u8>, String> {
    let manifest = BookManifest { version: BOOKS_VERSION, books: books.clone() };
    serde_json::to_vec(&manifest).map_err(|e| e.to_string())
}

/// The manifest envelope version, shared by every cloud transport.
pub const BOOKS_VERSION: u32 = 1;

/// The cloud file name for a book: fingerprint first (unique), then the
/// library name made path-safe (it can hold folders). The manifest maps
/// the fingerprint back, so the name is only a hint.
pub fn remote_book_name(fingerprint: &str, name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c })
        .collect();
    format!("book-{fingerprint}-{safe}")
}

/// Whether an id belongs to a cloud's secret (a refresh token, or the
/// phone's Google login waiting for the browser), kept in the same
/// credential store as catalog passwords: no catalog source may use one,
/// even one hand-written into the config.
pub fn is_token_id(id: &str) -> bool {
    id == crate::google::LOGIN_ID || stores().iter().any(|store| store.token_id() == id)
}

/// Uploads a book file the cloud manifest lacks, then records it: the
/// fingerprint means same bytes, so an entry already there means nothing to
/// do. The manifest is re-read before writing, so two devices uploading at
/// once keep both books. A manifest that fails to load stops the upload: an
/// empty one would overwrite every book recorded so far.
pub fn push_book_missing(
    fingerprint: &str,
    record: &BookRecord,
    bytes: &[u8],
    manifest: impl Fn() -> Result<Option<HashMap<String, BookRecord>>, String>,
    upload: impl Fn(&str, &[u8]) -> Result<(), String>,
    store: impl Fn(&HashMap<String, BookRecord>) -> Result<(), String>,
) -> Result<bool, String> {
    if manifest()?.is_some_and(|books| books.contains_key(fingerprint)) {
        return Ok(false);
    }
    upload(&remote_book_name(fingerprint, &record.name), bytes)?;
    let mut fresh = manifest()?.unwrap_or_default();
    fresh.entry(fingerprint.into()).or_insert_with(|| record.clone());
    store(&fresh)?;
    Ok(true)
}

/// Parses a token endpoint answer.
pub fn token_json(body: &[u8]) -> Result<serde_json::Value, String> {
    serde_json::from_slice(body).map_err(|e| format!("login: {e}"))
}

/// The access token in a token endpoint answer.
pub fn access_from(tokens: &serde_json::Value, service: &str) -> Result<String, String> {
    tokens["access_token"]
        .as_str()
        .ok_or_else(|| format!("login: {service} gave no access token"))
        .map(str::to_string)
}

/// The access token in a refresh answer and how many seconds it lives
/// (`expires_in`, or an hour when the answer doesn't say).
pub fn access_with_life(body: &[u8], service: &str) -> Result<(String, u64), String> {
    let tokens = token_json(body)?;
    Ok((access_from(&tokens, service)?, tokens["expires_in"].as_u64().unwrap_or(3600)))
}

/// A re-authorization may come back without a new refresh token (no consent
/// screen is shown): keep working with the one already kept instead of
/// failing the login.
pub fn pick_refresh(fresh: Option<String>, stored: Option<String>, service: &str) -> Result<String, String> {
    fresh
        .or(stored)
        .ok_or_else(|| format!("login: {service} gave no refresh token (ask for offline access again)"))
}

/// Both tokens after a code exchange: the access one, and the fresh refresh
/// one (or the kept one when the answer carries none).
pub fn tokens(body: &[u8], stored: Option<String>, service: &str) -> Result<(String, String), String> {
    let tokens = token_json(body)?;
    let access = access_from(&tokens, service)?;
    let fresh = tokens["refresh_token"].as_str().map(str::to_string);
    Ok((access, pick_refresh(fresh, stored, service)?))
}

/// One transport's last sync failure: a dead transport stays quiet, but the
/// settings show what went wrong instead of failing silently.
pub struct ErrorSlot(Mutex<Option<String>>);

impl ErrorSlot {
    /// An empty slot, for the transport's static.
    pub const fn new() -> Self {
        ErrorSlot(Mutex::new(None))
    }

    /// Remembers the outcome for the settings: failures stay visible there,
    /// successes clear them.
    pub fn track<T>(&self, result: Result<T, String>) -> Result<T, String> {
        *self.0.lock().unwrap() = result.as_ref().err().cloned();
        result
    }

    /// The last sync failure, if any.
    pub fn last(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(cfi: &str, time: u64) -> RemoteRecord {
        RemoteRecord { cfi: cfi.into(), fraction: 0.5, time, device: "phone".into(), device_id: "phone".into() }
    }

    fn position(cfi: &str, time: u64) -> crate::store::Position {
        crate::store::Position { cfi: cfi.into(), fraction: 0.5, time }
    }

    #[test]
    fn merge_newest_wins_and_ties_keep_local() {
        let mut local = HashMap::new();
        local.insert("new-remote".into(), position("a", 100));
        local.insert("new-local".into(), position("a", 300));
        local.insert("tie".into(), position("a", 200));
        local.insert("local-only".into(), position("a", 50));
        let mut remote = HashMap::new();
        remote.insert("new-remote".into(), record("b", 200));
        remote.insert("new-local".into(), record("b", 100));
        remote.insert("tie".into(), record("b", 200));
        remote.insert("remote-only".into(), record("c", 400));
        let merged = merge_for_upload(&local, &remote, "pc", "pc");
        // Remote newer: taken as is, writer kept for the echo check.
        assert_eq!(merged["new-remote"].cfi, "b");
        assert_eq!(merged["new-remote"].device_id, "phone");
        // Local newer or tied: stamped with this install.
        assert_eq!(merged["new-local"].cfi, "a");
        assert_eq!(merged["new-local"].device_id, "pc");
        assert_eq!(merged["tie"].cfi, "a");
        assert_eq!(merged["local-only"].cfi, "a");
        assert_eq!(merged["remote-only"].cfi, "c");
        assert_eq!(merged.len(), 5, "union of both sides");
        // The local-only book is news for the cloud.
        assert!(should_upload(&merged, &remote));
        // Nothing new: the merged file matches the cloud one.
        assert!(!should_upload(&merge_for_upload(&HashMap::new(), &remote, "pc", "pc"), &remote));
        let mut changed = merged.clone();
        changed.get_mut("tie").unwrap().cfi = "z".into();
        assert!(should_upload(&changed, &remote));
    }

    fn book_record(name: &str) -> BookRecord {
        BookRecord { name: name.into(), size: 10, sha256: "abc".into() }
    }

    #[test]
    fn manifest_round_trips_with_unknown_fields_tolerated() {
        let mut books = HashMap::new();
        books.insert("fp".into(), book_record("Author/Book.epub"));
        let back = parse_manifest(&render_manifest(&books).unwrap()).unwrap();
        assert_eq!(back["fp"], book_record("Author/Book.epub"));
        let future: BookManifest = serde_json::from_value(serde_json::json!({
            "version": 2,
            "books": { "fp": { "name": "a", "size": 1, "sha256": "b" } },
            "future": true,
        }))
        .unwrap();
        assert_eq!(future.books["fp"].name, "a");
    }

    #[test]
    fn positions_file_round_trips_with_unknown_fields_tolerated() {
        let mut positions = HashMap::new();
        positions.insert("fp".into(), record("a", 100));
        let json = serde_json::to_string(&SyncFile { version: POSITIONS_VERSION, positions }).unwrap();
        let back: SyncFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.positions["fp"], record("a", 100));
        let future: SyncFile = serde_json::from_value(serde_json::json!({
            "version": 2,
            "positions": { "fp": { "cfi": "a", "fraction": 0.5, "time": 100, "device": "phone", "deviceId": "phone" } },
            "future": true,
        }))
        .unwrap();
        assert_eq!(future.positions["fp"].cfi, "a");
    }

    #[test]
    fn an_access_token_is_kept_until_shortly_before_it_expires() {
        let cache = TokenCache::new();
        assert_eq!(cache.fresh(), None);
        cache.keep("t", 3600);
        assert_eq!(cache.fresh().as_deref(), Some("t"));
        cache.keep("short", 30);
        assert_eq!(cache.fresh(), None, "under a minute left counts as expired");
        cache.keep("t", 3600);
        cache.clear();
        assert_eq!(cache.fresh(), None);
        // A refresh answer says how long the token lives, or an hour.
        assert_eq!(access_with_life(br#"{"access_token":"a","expires_in":14400}"#, "X").unwrap(), ("a".into(), 14400));
        assert_eq!(access_with_life(br#"{"access_token":"a"}"#, "X").unwrap(), ("a".into(), 3600));
    }

    #[test]
    fn cloud_secrets_are_recognised_by_their_ids() {
        assert!(is_token_id("dropbox") && is_token_id("google") && is_token_id("google-login"));
        assert!(!is_token_id("1a2b"));
    }

    #[test]
    fn remote_names_are_unique_and_path_safe() {
        assert_eq!(remote_book_name("fp", "Author/Book.epub"), "book-fp-Author_Book.epub");
        assert_eq!(remote_book_name("fp", "a\\b"), "book-fp-a_b");
        assert_ne!(remote_book_name("fp1", "Book.epub"), remote_book_name("fp2", "Book.epub"));
    }
}
