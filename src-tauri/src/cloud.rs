//! What the cloud transports (Dropbox, Google Drive) share: one
//! fingerprint-keyed positions file, its newest-wins merge, the token
//! endpoint answers, and a slot for the last failure each transport shows
//! in the settings.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

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

/// The ids the clouds keep their refresh tokens under, in the same
/// credential store as catalog passwords.
const TOKEN_IDS: [&str; 2] = [crate::dropbox::TOKEN_ID, crate::google::TOKEN_ID];

/// Whether an id belongs to a cloud's token: no catalog source may use one,
/// even one hand-written into the config.
pub fn is_token_id(id: &str) -> bool {
    TOKEN_IDS.contains(&id)
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
    fn remote_names_are_unique_and_path_safe() {
        assert_eq!(remote_book_name("fp", "Author/Book.epub"), "book-fp-Author_Book.epub");
        assert_eq!(remote_book_name("fp", "a\\b"), "book-fp-a_b");
        assert_ne!(remote_book_name("fp1", "Book.epub"), remote_book_name("fp2", "Book.epub"));
    }
}
