//! Reading-progress sync through Google Drive: OAuth (PKCE over a loopback
//! address), one fingerprint-keyed positions file, newest-wins merge.
//!
//! The file lives in the app's hidden Drive folder (`appDataFolder`), so the
//! app never sees the rest of the Drive (the narrow `drive.appdata` scope).
//! Secrets stay out of the config: the refresh token is kept like catalog
//! passwords (the OS credential store, the Android Keystore).

use crate::cloud::{parse_manifest, render_manifest, BookRecord, ErrorSlot, RemoteRecord, SyncFile, access_from, merge_for_upload, should_upload, token_json, tokens};
use crate::platform::{Active, Platform};
use crate::{kosync, net, oauth};
use oauth::PendingLogin;
use std::collections::HashMap;
use std::sync::Mutex;

/// The last Google Drive sync failure, for the settings.
static LAST_ERROR: ErrorSlot = ErrorSlot::new();

/// The last sync failure, if any.
pub fn last_error() -> Option<String> {
    LAST_ERROR.last()
}

/// The Google Cloud OAuth client ID. Desktop: a Desktop client with the
/// `drive.appdata` scope. Phone: an Android client for the app's package and
/// release-key SHA-1. Baked in at build time from the environment (CI reads
/// them from repository secrets); empty in a plain checkout, in which case
/// every action refuses with how to set it up.
#[cfg(not(mobile))]
pub const CLIENT_ID: &str = match option_env!("GOOGLE_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
#[cfg(mobile)]
pub const CLIENT_ID: &str = match option_env!("GOOGLE_ANDROID_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// The desktop client's secret. Desktop clients with a secret must send it on
/// the token endpoint, even with PKCE. Same sourcing as the ID. The Android
/// client has no secret. Never committed: GitHub's push protection blocks it.
#[cfg(not(mobile))]
pub const CLIENT_SECRET: &str = match option_env!("GOOGLE_CLIENT_SECRET") {
    Some(secret) => secret,
    None => "",
};
#[cfg(mobile)]
pub const CLIENT_SECRET: &str = "";

/// Where the refresh token is kept, like catalog passwords.
const TOKEN_ID: &str = "google";

/// The positions file in the app folder; the envelope leaves room for
/// bookmarks and settings later (the same shape as the Dropbox one).
const FILE_NAME: &str = "sync-state.json";
const FILE_VERSION: u32 = 1;

/// The narrow Drive scope: the app's own hidden folder, nothing else.
const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";

/// The OAuth callback. Desktop: the loopback address (Desktop-type clients
/// accept any loopback port, so it needs no registration). Phone: the app's
/// own scheme, caught by an intent filter (see AndroidManifest.xml) — the
/// answer arrives as an intent instead of a loopback connection.
#[cfg(not(mobile))]
const REDIRECT_URI: &str = "http://localhost:51944/googledrive";
#[cfg(mobile)]
const REDIRECT_URI: &str = "com.olegg90.bookreader:/oauth2redirect";

/// The client ID and secret, or why Google Drive cannot work yet. The phone's
/// Android client has no secret; the desktop one must send its own.
fn credentials() -> Result<(&'static str, &'static str), String> {
    if CLIENT_ID.is_empty() || (!cfg!(mobile) && CLIENT_SECRET.is_empty()) {
        return Err(if cfg!(mobile) {
            "Google Drive is not set up yet: build with GOOGLE_ANDROID_CLIENT_ID in the environment (an Android OAuth client in the Google Cloud Console).".into()
        } else {
            "Google Drive is not set up yet: build with GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET in the environment (a Desktop OAuth client in the Google Cloud Console, drive.appdata scope).".into()
        });
    }
    Ok((CLIENT_ID, CLIENT_SECRET))
}

/// The Google authorize address for the loopback flow.
fn auth_url(client_id: &str, challenge: &str, state: &str) -> String {
    url::form_urlencoded::Serializer::new(String::from("https://accounts.google.com/o/oauth2/v2/auth?"))
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .finish()
}

/// Binds the loopback address and builds the Google authorize address.
pub fn begin() -> Result<PendingLogin, String> {
    let (client_id, _) = credentials()?;
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    let listener = oauth::bind(51944)?;
    let url = auth_url(client_id, &oauth::challenge(&verifier), &state);
    Ok(PendingLogin { verifier, state, listener, url })
}

/// Waits for the browser's code (or the timeout), trades it for tokens and
/// keeps the refresh token like a catalog password.
pub fn finish(login: PendingLogin) -> Result<(), String> {
    let code = oauth::wait_for_code(login.listener, &login.state, "Google")?;
    let (_access, refresh) = exchange(&code, &login.verifier, Active::password(TOKEN_ID))?;
    Active::save_password(TOKEN_ID, &refresh)
}

/// A login waiting for the phone's browser: the answer arrives as an intent
/// with the app's scheme, so nothing listens locally (unlike the desktop).
struct AndroidLogin {
    verifier: String,
    state: String,
}

static ANDROID_LOGIN: Mutex<Option<AndroidLogin>> = Mutex::new(None);

/// Starts the phone login: open the returned address in the browser, and the
/// answer comes back to `finish_answer` through the app's intent filter.
pub fn begin_android() -> Result<String, String> {
    let (client_id, _) = credentials()?;
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    let url = auth_url(client_id, &oauth::challenge(&verifier), &state);
    *ANDROID_LOGIN.lock().unwrap() = Some(AndroidLogin { verifier, state });
    Ok(url)
}

/// Finishes the phone login with the address the browser came back to.
pub fn finish_answer(answer: &str) -> Result<(), String> {
    let login = ANDROID_LOGIN.lock().unwrap().take().ok_or("login: no Google login is waiting")?;
    let code = oauth::code_in_answer(answer, &login.state, "Google")?;
    let (_access, refresh) = exchange(&code, &login.verifier, Active::password(TOKEN_ID))?;
    Active::save_password(TOKEN_ID, &refresh)
}

/// Whether Google Drive is connected (a refresh token is kept).
pub fn is_connected() -> bool {
    Active::has_password(TOKEN_ID)
}

/// Forgets the refresh token; connecting again starts over.
pub fn disconnect() {
    Active::forget_password(TOKEN_ID);
}

/// Trades an auth code (or a refresh token) for tokens.
fn exchange(code: &str, verifier: &str, stored: Option<String>) -> Result<(String, String), String> {
    let (client_id, client_secret) = credentials()?;
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("code", code)
        .append_pair("client_id", client_id)
        .append_pair("code_verifier", verifier)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("grant_type", "authorization_code");
    // The Android client has no secret; the desktop one must send its own.
    if !client_secret.is_empty() {
        form.append_pair("client_secret", client_secret);
    }
    let body = form.finish();
    let reply = net::post(
        "https://oauth2.googleapis.com/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Google login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    tokens(&reply.body, stored, "Google")
}

/// Trades the kept refresh token for an access token.
fn access_token() -> Result<String, String> {
    let refresh = Active::password(TOKEN_ID).ok_or_else(|| "Google Drive is not connected.".to_string())?;
    let (client_id, client_secret) = credentials()?;
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("refresh_token", &refresh)
        .append_pair("client_id", client_id)
        .append_pair("grant_type", "refresh_token");
    if !client_secret.is_empty() {
        form.append_pair("client_secret", client_secret);
    }
    let body = form.finish();
    let reply = net::post(
        "https://oauth2.googleapis.com/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Google login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    // A refresh answer carries no refresh token (only the code exchange does).
    access_from(&token_json(&reply.body)?, "Google")
}

/// The book manifest's name in the app folder.
const MANIFEST_NAME: &str = "sync-books.json";

/// The positions file's id in the app folder, if the app ever synced.
fn find_file(access: &str, name: &str) -> Result<Option<String>, String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("spaces", "appDataFolder")
        .append_pair("q", &format!("name = '{name}' and trashed = false"))
        .append_pair("fields", "files(id)")
        .append_pair("pageSize", "10")
        .finish();
    let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files?{query}"), access)?;
    if reply.status != 200 {
        return Err(format!("Google Drive list failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    let list: serde_json::Value = serde_json::from_slice(&reply.body).map_err(|e| format!("Google Drive list is not ours: {e}"))?;
    Ok(list["files"].as_array().and_then(|files| files.first()).and_then(|file| file["id"].as_str()).map(str::to_string))
}

/// Downloads an opaque cloud file by its id.
fn download_bytes(access: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files/{file_id}?alt=media"), access)?;
    if reply.status != 200 {
        return Err(format!("Google Drive download failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    Ok(reply.body)
}

/// Downloads the positions file.
fn download(access: &str, file_id: &str) -> Result<HashMap<String, RemoteRecord>, String> {
    let body = download_bytes(access, file_id)?;
    let file: SyncFile = serde_json::from_slice(&body).map_err(|e| format!("Google Drive file is not ours: {e}"))?;
    Ok(file.positions)
}

/// The multipart body for creating or replacing a file: its metadata (the
/// app folder on create, nothing on update) and its content.
fn multipart_body(
    metadata: serde_json::Value,
    data: Vec<u8>,
    content_type: &str,
) -> Result<(String, Vec<u8>), String> {
    let boundary = oauth::random_string(32);
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n").as_bytes());
    body.extend_from_slice(&serde_json::to_vec(&metadata).map_err(|e| e.to_string())?);
    body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/related; boundary={boundary}"), body))
}

/// Creates or replaces a file in the app folder.
fn store_bytes(
    access: &str,
    file_id: Option<&str>,
    metadata: serde_json::Value,
    data: Vec<u8>,
    content_type: &str,
) -> Result<(), String> {
    let (content_type, body) = multipart_body(metadata, data, content_type)?;
    let url = match file_id {
        Some(id) => format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=multipart"),
        None => "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart".into(),
    };
    let headers = [("Authorization", format!("Bearer {access}")), ("Content-Type", content_type)];
    let headers: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let reply = match file_id {
        Some(_) => net::patch(&url, &headers, &body)?,
        None => net::post(&url, &headers, &body)?,
    };
    if reply.status == 200 {
        Ok(())
    } else {
        Err(format!("Google Drive upload failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)))
    }
}

/// Uploads the merged positions file: created in the app folder the first
/// time, replaced afterwards.
fn upload(access: &str, file_id: Option<&str>, positions: &HashMap<String, RemoteRecord>) -> Result<(), String> {
    let metadata = match file_id {
        Some(_) => serde_json::json!({}),
        None => serde_json::json!({ "name": FILE_NAME, "parents": ["appDataFolder"] }),
    };
    let data = serde_json::to_vec(&SyncFile { version: FILE_VERSION, positions: positions.clone() }).map_err(|e| e.to_string())?;
    store_bytes(access, file_id, metadata, data, "application/json")
}

/// Uploads an opaque cloud file: created in the app folder the first time,
/// replaced afterwards.
fn upload_bytes(access: &str, file_id: Option<&str>, name: &str, data: &[u8]) -> Result<(), String> {
    let metadata = match file_id {
        Some(_) => serde_json::json!({}),
        None => serde_json::json!({ "name": name, "parents": ["appDataFolder"] }),
    };
    store_bytes(access, file_id, metadata, data.to_vec(), "application/octet-stream")
}

/// The cloud's book manifest, if the app ever synced books.
pub fn book_manifest() -> Option<HashMap<String, BookRecord>> {
    LAST_ERROR.track(manifest_map()).ok().flatten()
}

fn manifest_map() -> Result<Option<HashMap<String, BookRecord>>, String> {
    let access = access_token()?;
    let Some(id) = find_file(&access, MANIFEST_NAME)? else { return Ok(None) };
    let body = download_bytes(&access, &id)?;
    parse_manifest(&body).map(Some)
}

/// Stores the cloud book manifest.
fn store_manifest(books: &HashMap<String, BookRecord>) -> Result<(), String> {
    let access = access_token()?;
    let file_id = find_file(&access, MANIFEST_NAME)?;
    upload_bytes(&access, file_id.as_deref(), MANIFEST_NAME, &render_manifest(books)?)
}

/// Uploads a book file under its fingerprint name.
fn upload_book_file(remote_name: &str, bytes: &[u8]) -> Result<(), String> {
    let access = access_token()?;
    let file_id = find_file(&access, remote_name)?;
    upload_bytes(&access, file_id.as_deref(), remote_name, bytes)
}

/// Uploads a book file the cloud lacks, and records it in the manifest.
pub fn push_book(fingerprint: &str, record: &BookRecord, bytes: &[u8]) -> Result<bool, String> {
    LAST_ERROR.track(crate::cloud::push_book_missing(
        fingerprint,
        record,
        bytes,
        book_manifest,
        upload_book_file,
        store_manifest,
    ))
}

/// The open book's remote position, if any: a dead transport reports
/// nothing instead of failing the pull.
pub fn pull(fingerprint: &str) -> Option<kosync::Progress> {
    LAST_ERROR.track(pull_inner(fingerprint)).ok().flatten()
}

/// The cloud's whole positions file, if the app ever synced.
pub fn remote_positions() -> Option<HashMap<String, RemoteRecord>> {
    LAST_ERROR.track(remote_map()).ok().flatten()
}

fn remote_map() -> Result<Option<HashMap<String, RemoteRecord>>, String> {
    let access = access_token()?;
    match find_file(&access, FILE_NAME)? {
        Some(id) => Ok(Some(download(&access, &id)?)),
        None => Ok(None),
    }
}

fn pull_inner(fingerprint: &str) -> Result<Option<kosync::Progress>, String> {
    let remote = remote_map()?;
    // `time` is ms here like locally; KOSync counts seconds.
    Ok(remote.as_ref().and_then(|positions| positions.get(fingerprint)).map(|r| kosync::Progress {
        progress: r.cfi.clone(),
        percentage: r.fraction,
        device: r.device.clone(),
        device_id: r.device_id.clone(),
        timestamp: r.time / 1000,
    }))
}

/// Merges the local positions (with the open book's fresh one) over the
/// cloud file and uploads it when anything changed.
pub fn push(
    local: &HashMap<String, crate::store::Position>,
    fingerprint: &str,
    cfi: &str,
    fraction: f64,
    device: &str,
    device_id: &str,
) -> Result<(), String> {
    LAST_ERROR.track(push_inner(local, fingerprint, cfi, fraction, device, device_id))
}

fn push_inner(
    local: &HashMap<String, crate::store::Position>,
    fingerprint: &str,
    cfi: &str,
    fraction: f64,
    device: &str,
    device_id: &str,
) -> Result<(), String> {
    let access = access_token()?;
    let file_id = find_file(&access, FILE_NAME)?;
    let remote = match &file_id {
        Some(id) => download(&access, id)?,
        None => HashMap::new(),
    };
    let mut local = local.clone();
    local.insert(
        fingerprint.into(),
        crate::store::Position { cfi: cfi.into(), fraction, time: crate::clock::now_ms() },
    );
    let merged = merge_for_upload(&local, &remote, device, device_id);
    if !should_upload(&merged, &remote) {
        return Ok(());
    }
    upload(&access, file_id.as_deref(), &merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::pick_refresh;

    fn record(cfi: &str, time: u64) -> RemoteRecord {
        RemoteRecord { cfi: cfi.into(), fraction: 0.5, time, device: "phone".into(), device_id: "phone".into() }
    }

    #[test]
    fn client_credentials_or_setup_help() {
        // Baked in at build time; without them every action refuses with setup help.
        if CLIENT_ID.is_empty() || CLIENT_SECRET.is_empty() {
            assert!(credentials().is_err());
        } else {
            assert!(CLIENT_ID.ends_with(".apps.googleusercontent.com"));
            assert!(credentials().is_ok());
            assert!(auth_url(CLIENT_ID, "challenge", "state").contains(&format!("client_id={CLIENT_ID}")));
        }
    }

    #[test]
    fn auth_url_asks_for_a_code_with_pkce_and_offline_access() {
        let url = auth_url("client", "challenge", "state");
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("client_id=client"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A51944%2Fgoogledrive"));
        assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.appdata"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
    }

    #[test]
    fn refresh_answer_carries_no_refresh_token() {
        // What oauth2.googleapis.com answers to a refresh grant: access only.
        let body = br#"{"access_token": "ya29.XXXX", "expires_in": 3599, "scope": "https://www.googleapis.com/auth/drive.appdata", "token_type": "Bearer"}"#;
        assert_eq!(access_from(&token_json(body).unwrap(), "Google").unwrap(), "ya29.XXXX");
        assert!(tokens(body, None, "Google").is_err(), "a code exchange without one still fails");
        assert_eq!(tokens(body, Some("old".into()), "Google"), Ok(("ya29.XXXX".into(), "old".into())));
    }

    #[test]
    fn reauth_without_a_new_token_keeps_the_kept_one() {
        assert_eq!(pick_refresh(Some("new".into()), Some("old".into()), "Google"), Ok("new".into()));
        assert_eq!(pick_refresh(None, Some("old".into()), "Google"), Ok("old".into()));
        assert!(pick_refresh(None, None, "Google").is_err());
    }

    #[test]
    fn file_round_trips_with_unknown_fields_tolerated() {
        let mut positions = HashMap::new();
        positions.insert("fp".into(), record("a", 100));
        let file = SyncFile { version: FILE_VERSION, positions };
        let json = serde_json::to_string(&file).unwrap();
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
    fn multipart_body_carries_metadata_and_content() {
        let data = br#"{"positions":{}}"#.to_vec();
        let metadata = serde_json::json!({ "name": "sync-state.json", "parents": ["appDataFolder"] });
        // First upload: the file is created in the app folder.
        let (content_type, body) = multipart_body(metadata, data.clone(), "application/json").unwrap();
        let boundary = content_type.strip_prefix("multipart/related; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains(r#""name":"sync-state.json""#));
        assert!(text.contains(r#""parents":["appDataFolder"]"#));
        assert!(text.contains(r#""positions":{}"#));
        assert!(text.contains("Content-Type: application/json"));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
        // Later uploads replace the file: no metadata needed.
        let (_, body) = multipart_body(serde_json::json!({}), data, "application/octet-stream").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(!text.contains("appDataFolder"), "{text}");
        assert!(text.contains("Content-Type: application/octet-stream"));
    }
}
