//! Reading-progress sync through Google Drive: OAuth (PKCE over a loopback
//! address), one fingerprint-keyed positions file, newest-wins merge.
//!
//! The file lives in the app's hidden Drive folder (`appDataFolder`), so the
//! app never sees the rest of the Drive (the narrow `drive.appdata` scope).
//! Secrets stay out of the config: the refresh token is kept like catalog
//! passwords (the OS credential store, the Android Keystore).

use crate::platform::{Active, Platform};
use crate::{kosync, net, oauth};
use oauth::PendingLogin;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// The last sync failure, if any: a dead transport stays quiet, but the
/// settings show what went wrong instead of failing silently.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Remembers the outcome for the settings: failures stay visible there,
/// successes clear them.
fn track<T>(result: Result<T, String>) -> Result<T, String> {
    *LAST_ERROR.lock().unwrap() = result.as_ref().err().cloned();
    result
}

/// The last sync failure, if any.
pub fn last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap().clone()
}

/// The Google Cloud OAuth client ID (a Desktop client with the
/// `drive.appdata` scope). Baked in at build time from the `GOOGLE_CLIENT_ID`
/// environment (CI reads it from a repository secret); empty in a plain
/// checkout, in which case every action refuses with how to set it up.
pub const CLIENT_ID: &str = match option_env!("GOOGLE_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

/// The same client's secret. Same sourcing as the ID (`GOOGLE_CLIENT_SECRET`):
/// Desktop clients with a secret must send it on the token endpoint, even
/// with PKCE. Never committed: GitHub's push protection blocks it.
pub const CLIENT_SECRET: &str = match option_env!("GOOGLE_CLIENT_SECRET") {
    Some(secret) => secret,
    None => "",
};

/// Where the refresh token is kept, like catalog passwords.
const TOKEN_ID: &str = "google";

/// The positions file in the app folder; the envelope leaves room for
/// bookmarks and settings later (the same shape as the Dropbox one).
const FILE_NAME: &str = "sync-state.json";
const FILE_VERSION: u32 = 1;

/// The narrow Drive scope: the app's own hidden folder, nothing else.
const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";

/// The OAuth callback. Desktop-type clients accept any loopback port, so this
/// needs no registration in the Cloud Console. No manifest or key-hash
/// changes are needed: the loopback address works in the desktop browser and
/// on the phone.
const REDIRECT_URI: &str = "http://localhost:51944/googledrive";

/// One book's synced position as the cloud file keeps it: the local shape
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

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncFile {
    version: u32,
    #[serde(default)]
    positions: HashMap<String, RemoteRecord>,
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

/// The client ID and secret, or why Google Drive cannot work yet.
fn credentials() -> Result<(&'static str, &'static str), String> {
    if CLIENT_ID.is_empty() || CLIENT_SECRET.is_empty() {
        return Err("Google Drive is not set up yet: build with GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET in the environment (a Desktop OAuth client in the Google Cloud Console, drive.appdata scope).".into());
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
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", code)
        .append_pair("client_id", client_id)
        .append_pair("client_secret", client_secret)
        .append_pair("code_verifier", verifier)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("grant_type", "authorization_code")
        .finish();
    let reply = net::post(
        "https://oauth2.googleapis.com/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Google login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    tokens(&reply.body, stored)
}

/// Trades the kept refresh token for an access token.
fn access_token() -> Result<String, String> {
    let refresh = Active::password(TOKEN_ID).ok_or_else(|| "Google Drive is not connected.".to_string())?;
    let (client_id, client_secret) = credentials()?;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("refresh_token", &refresh)
        .append_pair("client_id", client_id)
        .append_pair("client_secret", client_secret)
        .append_pair("grant_type", "refresh_token")
        .finish();
    let reply = net::post(
        "https://oauth2.googleapis.com/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Google login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    // A refresh answer carries no refresh token (only the code exchange does).
    access_from(&token_json(&reply.body)?)
}

fn token_json(body: &[u8]) -> Result<serde_json::Value, String> {
    serde_json::from_slice(body).map_err(|e| format!("login: {e}"))
}

fn access_from(tokens: &serde_json::Value) -> Result<String, String> {
    tokens["access_token"]
        .as_str()
        .ok_or_else(|| "login: Google gave no access token".to_string())
        .map(str::to_string)
}

/// A re-authorization may come back without a new refresh token (no consent
/// screen is shown): keep working with the one already kept instead of
/// failing the login.
fn pick_refresh(fresh: Option<String>, stored: Option<String>) -> Result<String, String> {
    fresh.or(stored).ok_or_else(|| "login: Google gave no refresh token (ask for offline access again)".into())
}

fn tokens(body: &[u8], stored: Option<String>) -> Result<(String, String), String> {
    let tokens = token_json(body)?;
    let access = access_from(&tokens)?;
    let fresh = tokens["refresh_token"].as_str().map(str::to_string);
    Ok((access, pick_refresh(fresh, stored)?))
}

/// The positions file's id in the app folder, if the app ever synced.
fn find_file(access: &str) -> Result<Option<String>, String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("spaces", "appDataFolder")
        .append_pair("q", "name = 'sync-state.json' and trashed = false")
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

/// Downloads the positions file.
fn download(access: &str, file_id: &str) -> Result<HashMap<String, RemoteRecord>, String> {
    let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files/{file_id}?alt=media"), access)?;
    if reply.status != 200 {
        return Err(format!("Google Drive download failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    let file: SyncFile = serde_json::from_slice(&reply.body).map_err(|e| format!("Google Drive file is not ours: {e}"))?;
    Ok(file.positions)
}

/// The multipart body for creating or replacing the positions file: its
/// metadata (the app folder on create, nothing on update) and its content.
fn multipart_body(file_id: Option<&str>, positions: &HashMap<String, RemoteRecord>) -> Result<(String, Vec<u8>), String> {
    let metadata = match file_id {
        Some(_) => serde_json::json!({}),
        None => serde_json::json!({ "name": FILE_NAME, "parents": ["appDataFolder"] }),
    };
    let data = serde_json::to_vec(&SyncFile { version: FILE_VERSION, positions: positions.clone() }).map_err(|e| e.to_string())?;
    let boundary = oauth::random_string(32);
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n").as_bytes());
    body.extend_from_slice(&serde_json::to_vec(&metadata).map_err(|e| e.to_string())?);
    body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n").as_bytes());
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/related; boundary={boundary}"), body))
}

/// Uploads the merged positions file: created in the app folder the first
/// time, replaced afterwards.
fn upload(access: &str, file_id: Option<&str>, positions: &HashMap<String, RemoteRecord>) -> Result<(), String> {
    let (content_type, body) = multipart_body(file_id, positions)?;
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

/// The open book's remote position, if any: a dead transport reports
/// nothing instead of failing the pull.
pub fn pull(fingerprint: &str) -> Option<kosync::Progress> {
    track(pull_inner(fingerprint)).ok().flatten()
}

fn pull_inner(fingerprint: &str) -> Result<Option<kosync::Progress>, String> {
    let access = access_token()?;
    let remote = match find_file(&access)? {
        Some(id) => download(&access, &id)?,
        None => return Ok(None),
    };
    // `time` is ms here like locally; KOSync counts seconds.
    Ok(remote.get(fingerprint).map(|r| kosync::Progress {
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
    track(push_inner(local, fingerprint, cfi, fraction, device, device_id))
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
    let file_id = find_file(&access)?;
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

    fn record(cfi: &str, time: u64) -> RemoteRecord {
        RemoteRecord { cfi: cfi.into(), fraction: 0.5, time, device: "phone".into(), device_id: "phone".into() }
    }

    fn position(cfi: &str, time: u64) -> crate::store::Position {
        crate::store::Position { cfi: cfi.into(), fraction: 0.5, time }
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
        assert_eq!(access_from(&token_json(body).unwrap()).unwrap(), "ya29.XXXX");
        assert!(tokens(body, None).is_err(), "a code exchange without one still fails");
        assert_eq!(tokens(body, Some("old".into())), Ok(("ya29.XXXX".into(), "old".into())));
    }

    #[test]
    fn reauth_without_a_new_token_keeps_the_kept_one() {
        assert_eq!(pick_refresh(Some("new".into()), Some("old".into())), Ok("new".into()));
        assert_eq!(pick_refresh(None, Some("old".into())), Ok("old".into()));
        assert!(pick_refresh(None, None).is_err());
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
        assert_eq!(merged["new-remote"].cfi, "b");
        assert_eq!(merged["new-remote"].device_id, "phone");
        assert_eq!(merged["new-local"].cfi, "a");
        assert_eq!(merged["new-local"].device_id, "pc");
        assert_eq!(merged["tie"].cfi, "a");
        assert_eq!(merged["local-only"].cfi, "a");
        assert_eq!(merged["remote-only"].cfi, "c");
        assert_eq!(merged.len(), 5, "union of both sides");
        assert!(should_upload(&merged, &remote));
        assert!(!should_upload(&merge_for_upload(&HashMap::new(), &remote, "pc", "pc"), &remote));
    }

    #[test]
    fn multipart_body_carries_metadata_and_content() {
        let mut positions = HashMap::new();
        positions.insert("fp".into(), record("a", 100));
        // First upload: the file is created in the app folder.
        let (content_type, body) = multipart_body(None, &positions).unwrap();
        let boundary = content_type.strip_prefix("multipart/related; boundary=").unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains(r#""name":"sync-state.json""#));
        assert!(text.contains(r#""parents":["appDataFolder"]"#));
        assert!(text.contains(r#""cfi":"a""#));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
        // Later uploads replace the file: no metadata needed.
        let (_, body) = multipart_body(Some("id"), &positions).unwrap();
        let text = String::from_utf8(body).unwrap();
        assert!(!text.contains("appDataFolder"), "{text}");
        assert!(text.contains(r#""cfi":"a""#));
    }
}
