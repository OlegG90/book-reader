//! What the cloud transports (Dropbox, Google Drive) share. Each cloud is a
//! `CloudStore`: a flat folder of named files behind an access token. On top
//! of that, written once here: the fingerprint-keyed positions file and its
//! newest-wins merge, the book manifest and book files, the access token
//! kept until it expires, and a slot for the last failure each cloud shows
//! in the settings.

use crate::kosync;
use crate::platform::{Active, Platform};
use crate::store::Position;
use crate::sync_error::{Kind, SyncError};
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
    fn refresh(&self, refresh_token: &str) -> Result<(String, u64), SyncError>;
    /// A file's bytes by name; `None` when the cloud has no such file.
    fn get(&self, access: &str, name: &str) -> Result<Option<Vec<u8>>, SyncError>;
    /// Writes a file by name, creating it or replacing what is there.
    fn put(&self, access: &str, name: &str, bytes: &[u8]) -> Result<(), SyncError>;
    /// The names of the files whose name starts with `prefix`.
    fn list(&self, access: &str, prefix: &str) -> Result<Vec<String>, SyncError>;
    /// The cloud's last failure, for the settings.
    fn errors(&self) -> &ErrorSlot;
    /// The access token while it lives, so one sync round asks for it once.
    fn tokens(&self) -> &TokenCache;

    /// Whether a refresh token is kept (a login happened).
    fn connected(&self) -> bool {
        Active::has_password(self.token_id())
    }

    /// An access token: the kept one while it lives, else a fresh one.
    fn access_token(&self) -> Result<String, SyncError> {
        if let Some(token) = self.tokens().fresh() {
            return Ok(token);
        }
        let refresh = Active::password(self.token_id())
            .ok_or_else(|| SyncError::new(Kind::NotConnected, format!("{} is not connected.", self.label())))?;
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

/// Each device writes only its own files in a cloud's app folder, so two
/// devices never overwrite each other: `positions-<device id>.json` and
/// `books-<device id>.json`. Readers merge every device's file.
const POSITIONS_PREFIX: &str = "positions-";
const BOOKS_PREFIX: &str = "books-";
/// The shared files earlier versions wrote; still read, so nothing they
/// hold is lost, and never written again.
const LEGACY_POSITIONS: &str = "sync-state.json";
const LEGACY_BOOKS: &str = "sync-books.json";
/// The positions file's envelope version.
const POSITIONS_VERSION: u32 = 1;
/// The book manifest's envelope version.
const BOOKS_VERSION: u32 = 1;

/// A device's own file of a kind.
fn device_file(prefix: &str, device_id: &str) -> String {
    format!("{prefix}{device_id}.json")
}

/// Every file of a kind in the cloud: each device's, then the legacy one.
fn files_of(store: &dyn CloudStore, access: &str, prefix: &str, legacy: &str) -> Result<Vec<String>, SyncError> {
    let mut names: Vec<String> = store.list(access, prefix)?.into_iter().filter(|n| n.starts_with(prefix) && n.ends_with(".json")).collect();
    names.sort();
    names.dedup();
    names.push(legacy.into());
    Ok(names)
}

/// Every cloud, in merge order.
pub fn stores() -> [&'static dyn CloudStore; 2] {
    [&crate::dropbox::STORE, &crate::google::STORE]
}

/// Forgets a cloud's login: the refresh token, the access token, and the
/// last failure the settings showed for it.
pub fn disconnect(store: &dyn CloudStore) {
    Active::forget_password(store.token_id());
    store.tokens().clear();
    let _ = store.errors().track::<(), SyncError>(Ok(()));
}

/// Keeps the refresh token of a new login, then drops an access token of an
/// earlier account (in this order, so a sync running meanwhile can't cache
/// the old account's token after the drop).
pub fn keep_login(store: &dyn CloudStore, refresh: &str) -> Result<(), String> {
    Active::save_password(store.token_id(), refresh)?;
    store.tokens().clear();
    Ok(())
}

/// Records a cloud call's outcome for the settings. A cloud without a login
/// has nothing to report. A rejected login also drops the kept access token,
/// so the next call logs in again and says what is wrong instead of reusing
/// a revoked token until it expires.
fn settle<T>(store: &dyn CloudStore, result: Result<T, SyncError>) -> Result<T, SyncError> {
    match &result {
        Err(e) if e.kind == Kind::NotConnected => return result,
        Err(e) if e.kind == Kind::Rejected => store.tokens().clear(),
        _ => {}
    }
    store.errors().track(result)
}

/// One positions file's records.
fn parse_positions(store: &dyn CloudStore, body: &[u8]) -> Result<HashMap<String, RemoteRecord>, SyncError> {
    let file: SyncFile = serde_json::from_slice(body).map_err(|e| SyncError::data(format!("{} file is not ours: {e}", store.label())))?;
    Ok(file.positions)
}

/// Every device's positions, merged newest per book: `None` when no device
/// ever wrote one. Another device's broken file is passed over, so it never
/// stops the rest from syncing.
fn all_positions(store: &dyn CloudStore, access: &str) -> Result<Option<HashMap<String, RemoteRecord>>, SyncError> {
    let mut merged: Option<HashMap<String, RemoteRecord>> = None;
    for name in files_of(store, access, POSITIONS_PREFIX, LEGACY_POSITIONS)? {
        let Some(body) = store.get(access, &name)? else { continue };
        let Ok(records) = parse_positions(store, &body) else { continue };
        let all = merged.get_or_insert_with(HashMap::new);
        for (fingerprint, record) in records {
            if all.get(&fingerprint).is_none_or(|have| record.time > have.time) {
                all.insert(fingerprint, record);
            }
        }
    }
    Ok(merged)
}

/// The cloud's positions from every device, if any device ever synced; a
/// dead cloud reports nothing (its failure goes to the settings).
pub fn remote_positions(store: &dyn CloudStore) -> Option<HashMap<String, RemoteRecord>> {
    settle(store, store.access_token().and_then(|access| all_positions(store, &access))).ok().flatten()
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

/// Writes this device's positions file: every local position, with the open
/// book's new one, stamped with this device. Nothing is uploaded when the
/// cloud already has exactly that.
pub fn push(
    store: &dyn CloudStore,
    local: &HashMap<String, Position>,
    fingerprint: &str,
    cfi: &str,
    fraction: f64,
    device: &str,
    device_id: &str,
) -> Result<(), SyncError> {
    let pushed = (|| {
        let access = store.access_token()?;
        let mut local = local.clone();
        local.insert(fingerprint.into(), Position { cfi: cfi.into(), fraction, time: crate::clock::now_ms() });
        let mine = own_records(&local, device, device_id);
        let name = device_file(POSITIONS_PREFIX, device_id);
        let kept = store.get(&access, &name)?.and_then(|body| parse_positions(store, &body).ok());
        if kept.as_ref() == Some(&mine) {
            return Ok(());
        }
        let body = serde_json::to_vec(&SyncFile { version: POSITIONS_VERSION, positions: mine }).map_err(|e| SyncError::data(e.to_string()))?;
        store.put(&access, &name, &body)
    })();
    settle(store, pushed)
}

/// One manifest file's books.
fn manifest_books(body: &[u8]) -> Result<HashMap<String, BookRecord>, SyncError> {
    parse_manifest(body).map_err(SyncError::data)
}

/// The books every device recorded: `None` when no device ever uploaded
/// one. Another device's broken manifest is passed over.
fn all_books(store: &dyn CloudStore, access: &str) -> Result<Option<HashMap<String, BookRecord>>, SyncError> {
    let mut union: Option<HashMap<String, BookRecord>> = None;
    for name in files_of(store, access, BOOKS_PREFIX, LEGACY_BOOKS)? {
        let Some(body) = store.get(access, &name)? else { continue };
        let Ok(books) = manifest_books(&body) else { continue };
        let all = union.get_or_insert_with(HashMap::new);
        for (fingerprint, record) in books {
            all.entry(fingerprint).or_insert(record);
        }
    }
    Ok(union)
}

/// The cloud's books from every device: `None` when no book was ever
/// uploaded, an error when the cloud could not be read (never mistaken for
/// an empty one).
pub fn manifest(store: &dyn CloudStore) -> Result<Option<HashMap<String, BookRecord>>, SyncError> {
    all_books(store, &store.access_token()?)
}

/// The cloud's books, if the app ever synced books; a failure goes to the settings.
pub fn book_manifest(store: &dyn CloudStore) -> Option<HashMap<String, BookRecord>> {
    settle(store, manifest(store)).ok().flatten()
}

/// A book file by its cloud name; `None` when the cloud never saw it.
pub fn download_book(store: &dyn CloudStore, remote_name: &str) -> Result<Option<Vec<u8>>, SyncError> {
    settle(store, store.access_token().and_then(|access| store.get(&access, remote_name)))
}

/// Uploads a book file the cloud lacks and records it in this device's
/// manifest; true when the cloud gained the book.
pub fn push_book(store: &dyn CloudStore, device_id: &str, fingerprint: &str, record: &BookRecord, bytes: &[u8]) -> Result<bool, SyncError> {
    let own = device_file(BOOKS_PREFIX, device_id);
    let pushed = store.access_token().and_then(|access| {
        push_book_missing(
            fingerprint,
            record,
            bytes,
            || all_books(store, &access),
            |name, bytes| store.put(&access, name, bytes),
            || store.get(&access, &own)?.map(|body| manifest_books(&body)).transpose(),
            |books| store.put(&access, &own, &render_manifest(books).map_err(SyncError::data)?),
        )
    });
    settle(store, pushed)
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
struct SyncFile {
    pub version: u32,
    #[serde(default)]
    pub positions: HashMap<String, RemoteRecord>,
}

/// This device's records for its positions file: every local position,
/// stamped with this device (the echo check needs a writer).
fn own_records(local: &HashMap<String, Position>, device: &str, device_id: &str) -> HashMap<String, RemoteRecord> {
    local
        .iter()
        .map(|(fingerprint, pos)| {
            let record = RemoteRecord { cfi: pos.cfi.clone(), fraction: pos.fraction, time: pos.time, device: device.into(), device_id: device_id.into() };
            (fingerprint.clone(), record)
        })
        .collect()
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
struct BookManifest {
    pub version: u32,
    #[serde(default)]
    pub books: HashMap<String, BookRecord>,
}

/// Parses a cloud book manifest.
fn parse_manifest(bytes: &[u8]) -> Result<HashMap<String, BookRecord>, String> {
    let manifest: BookManifest =
        serde_json::from_slice(bytes).map_err(|e| format!("book manifest is not ours: {e}"))?;
    Ok(manifest.books)
}

/// Renders a cloud book manifest.
fn render_manifest(books: &HashMap<String, BookRecord>) -> Result<Vec<u8>, String> {
    let manifest = BookManifest { version: BOOKS_VERSION, books: books.clone() };
    serde_json::to_vec(&manifest).map_err(|e| e.to_string())
}

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

/// Uploads a book file no device recorded yet, then adds it to this
/// device's own manifest: the fingerprint means same bytes, so an entry
/// already there means nothing to do. Only this device writes its manifest,
/// so two devices uploading at once never lose each other's books. Its own
/// manifest failing to load stops the upload: an empty one would drop every
/// book it recorded so far.
pub fn push_book_missing(
    fingerprint: &str,
    record: &BookRecord,
    bytes: &[u8],
    recorded: impl Fn() -> Result<Option<HashMap<String, BookRecord>>, SyncError>,
    upload: impl Fn(&str, &[u8]) -> Result<(), SyncError>,
    own: impl Fn() -> Result<Option<HashMap<String, BookRecord>>, SyncError>,
    store_own: impl Fn(&HashMap<String, BookRecord>) -> Result<(), SyncError>,
) -> Result<bool, SyncError> {
    if recorded()?.is_some_and(|books| books.contains_key(fingerprint)) {
        return Ok(false);
    }
    let mut mine = own()?.unwrap_or_default();
    upload(&remote_book_name(fingerprint, &record.name), bytes)?;
    mine.insert(fingerprint.into(), record.clone());
    store_own(&mine)?;
    Ok(true)
}

/// Posts a form to an OAuth token endpoint and returns the answer's body;
/// a refusal reads "<service> login failed (status): …". A 400 there means
/// the refresh token was revoked or expired (`invalid_grant`), so it counts
/// as a rejected login like 401.
pub fn token_post(url: &str, form: &str, service: &str) -> Result<Vec<u8>, SyncError> {
    let reply = crate::net::post(url, &[("Content-Type", "application/x-www-form-urlencoded")], form.as_bytes()).map_err(SyncError::offline)?;
    match reply.status {
        200 => Ok(reply.body),
        400 => Err(SyncError { kind: Kind::Rejected, ..SyncError::from_reply(&format!("{service} login"), &reply) }),
        _ => Err(SyncError::from_reply(&format!("{service} login"), &reply)),
    }
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
    pub fn track<T, E: std::fmt::Display>(&self, result: Result<T, E>) -> Result<T, E> {
        *self.0.lock().unwrap() = result.as_ref().err().map(E::to_string);
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
    use crate::sync_error::Kind;

    fn record(cfi: &str, time: u64) -> RemoteRecord {
        RemoteRecord { cfi: cfi.into(), fraction: 0.5, time, device: "phone".into(), device_id: "phone".into() }
    }

    fn position(cfi: &str, time: u64) -> crate::store::Position {
        crate::store::Position { cfi: cfi.into(), fraction: 0.5, time }
    }

    #[test]
    fn a_device_writes_its_own_positions_stamped_with_itself() {
        let mut local = HashMap::new();
        local.insert("fp".into(), position("a", 100));
        let mine = own_records(&local, "PC", "pc");
        assert_eq!((mine["fp"].cfi.as_str(), mine["fp"].device_id.as_str(), mine["fp"].time), ("a", "pc", 100));
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

    /// A cloud whose every file call fails with `kind`.
    struct Failing {
        kind: Kind,
        errors: ErrorSlot,
        tokens: TokenCache,
    }

    impl CloudStore for Failing {
        fn id(&self) -> &'static str {
            "failing"
        }
        fn module(&self) -> &'static str {
            "capability:failing"
        }
        fn token_id(&self) -> &'static str {
            "failing"
        }
        fn label(&self) -> &'static str {
            "Failing"
        }
        fn refresh(&self, _refresh_token: &str) -> Result<(String, u64), SyncError> {
            Ok(("token".into(), 3600))
        }
        fn access_token(&self) -> Result<String, SyncError> {
            if self.kind == Kind::NotConnected {
                return Err(SyncError::new(Kind::NotConnected, "Failing is not connected."));
            }
            Ok("token".into())
        }
        fn get(&self, _access: &str, _name: &str) -> Result<Option<Vec<u8>>, SyncError> {
            Err(SyncError::new(self.kind, "nope"))
        }
        fn put(&self, _access: &str, _name: &str, _bytes: &[u8]) -> Result<(), SyncError> {
            Err(SyncError::new(self.kind, "nope"))
        }
        fn list(&self, _access: &str, _prefix: &str) -> Result<Vec<String>, SyncError> {
            Err(SyncError::new(self.kind, "nope"))
        }
        fn errors(&self) -> &ErrorSlot {
            &self.errors
        }
        fn tokens(&self) -> &TokenCache {
            &self.tokens
        }
    }

    fn failing(kind: Kind) -> Failing {
        let store = Failing { kind, errors: ErrorSlot::new(), tokens: TokenCache::new() };
        store.tokens.keep("kept", 3600);
        store
    }

    #[test]
    fn each_kind_of_failure_is_handled_its_own_way() {
        // No login: nothing to report, nothing to retry.
        let store = failing(Kind::NotConnected);
        assert_eq!(push(&store, &HashMap::new(), "fp", "a", 0.5, "pc", "pc").unwrap_err().kind, Kind::NotConnected);
        assert_eq!(store.errors.last(), None);
        // A turned-down login drops the kept token, so the next call logs in again.
        let store = failing(Kind::Rejected);
        assert!(remote_positions(&store).is_none());
        assert_eq!(store.tokens.fresh(), None);
        assert_eq!(store.errors.last().as_deref(), Some("nope"));
        // No network: the token stays, the failure shows.
        let store = failing(Kind::Offline);
        assert!(download_book(&store, "book").is_err());
        assert_eq!(store.tokens.fresh().as_deref(), Some("kept"));
        assert_eq!(store.errors.last().as_deref(), Some("nope"));
    }

    #[test]
    fn failures_are_remembered_until_a_success() {
        let slot = ErrorSlot::new();
        assert_eq!(slot.last(), None);
        let _ = slot.track::<(), String>(Err("gone".into()));
        assert_eq!(slot.last().as_deref(), Some("gone"));
        let _ = slot.track::<(), String>(Ok(()));
        assert_eq!(slot.last(), None);
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
