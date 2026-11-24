//! Reading-progress sync through Dropbox: OAuth (PKCE over a loopback
//! address), one fingerprint-keyed positions file, newest-wins merge.
//!
//! The file lives in the app's own Dropbox folder (`/sync-state.json`), so
//! the app never sees the rest of the Dropbox. Secrets stay out of the
//! config: the refresh token is kept like catalog passwords (the OS
//! credential store, the Android Keystore).

use crate::platform::{Active, Platform};
use crate::{kosync, net};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// The Dropbox app key: the "BookReader Sync" scoped app in the App Console
/// with app-folder access (files.content.read + files.content.write).
pub const APP_KEY: &str = "hq54wf951whc1jv";

/// Where the refresh token is kept, like catalog passwords.
const TOKEN_ID: &str = "dropbox";

/// The positions file in the app's Dropbox folder; the envelope leaves room
/// for bookmarks and settings later.
const FILE_PATH: &str = "/sync-state.json";
const FILE_VERSION: u32 = 1;

/// The OAuth callback: register exactly this URI in the App Console
/// (Dropbox requires an exact match; "Allow public clients" must be on,
/// as the app has no secret). No manifest or key-hash changes are needed:
/// the loopback address works in the desktop browser and on the phone.
const REDIRECT_URI: &str = "http://localhost:51943/dropbox";

/// How long `connect` waits for the browser to come back with the code.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

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

/// An OAuth login waiting for the browser: open `url`, then `finish` it.
pub struct PendingLogin {
    verifier: String,
    state: String,
    listener: TcpListener,
    pub url: String,
}

/// The app key, or why Dropbox cannot work yet.
fn app_key() -> Result<&'static str, String> {
    if APP_KEY.is_empty() {
        return Err("Dropbox is not set up yet: create a scoped app (app-folder access) in the Dropbox App Console and put its key in dropbox::APP_KEY.".into());
    }
    Ok(APP_KEY)
}

fn random_string(len: usize) -> String {
    use rand::{distr::Alphanumeric, Rng};
    rand::rng().sample_iter(&Alphanumeric).take(len).map(char::from).collect()
}

/// The S256 challenge for a verifier (RFC 7636 test vector covered below).
fn challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The Dropbox authorize address for the loopback flow.
fn auth_url(app_key: &str, challenge: &str, state: &str) -> String {
    url::form_urlencoded::Serializer::new(String::from("https://www.dropbox.com/oauth2/authorize?"))
        .append_pair("response_type", "code")
        .append_pair("client_id", app_key)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("state", state)
        .append_pair("token_access_type", "offline")
        .finish()
}

/// Binds the loopback address (dual-stack where possible, else IPv4) and
/// builds the Dropbox authorize address. The port is fixed: it must match
/// the URI registered in the App Console.
pub fn begin() -> Result<PendingLogin, String> {
    let app_key = app_key()?;
    let verifier = random_string(64);
    let state = random_string(16);
    // `localhost` may resolve to ::1 or 127.0.0.1 depending on the device;
    // listen on both where the stack allows it, so the browser always lands.
    let listener = TcpListener::bind("[::]:51943")
        .or_else(|_| TcpListener::bind("127.0.0.1:51943"))
        .map_err(|e| format!("login: the redirect port is busy ({e})"))?;
    let url = auth_url(app_key, &challenge(&verifier), &state);
    Ok(PendingLogin { verifier, state, listener, url })
}

/// Waits for the browser's code (or the timeout), trades it for tokens and
/// keeps the refresh token like a catalog password.
pub fn finish(login: PendingLogin) -> Result<(), String> {
    let code = wait_for_code(login.listener, &login.state)?;
    let (_access, refresh) = exchange(app_key()?, &code, &login.verifier)?;
    Active::save_password(TOKEN_ID, &refresh)
}

/// Whether Dropbox is connected (a refresh token is kept).
pub fn is_connected() -> bool {
    Active::has_password(TOKEN_ID)
}

/// Forgets the refresh token; connecting again starts over.
pub fn disconnect() {
    Active::forget_password(TOKEN_ID);
}

/// Waits up to `LOGIN_TIMEOUT` for the browser to come back with `?code=`
/// (or an error, when the reader declines in the browser).
fn wait_for_code(listener: TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| format!("login: {e}"))?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => return read_code(&stream, state),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if start.elapsed() > LOGIN_TIMEOUT {
                    return Err("Timed out waiting for Dropbox in the browser.".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("login: {e}")),
        }
    }
}

/// Reads the browser's answer (`GET /?code=…&state=…`), checks it answers
/// this login, and tells the tab it can close.
fn read_code(stream: &std::net::TcpStream, state: &str) -> Result<String, String> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| format!("login: {e}"))?;
    let mut first = String::new();
    BufReader::new(stream).read_line(&mut first).map_err(|e| format!("login: {e}"))?;
    let target = first.split_whitespace().nth(1).ok_or("login: the browser sent nothing usable")?;
    let query: HashMap<_, _> =
        url::Url::parse(&format!("http://localhost{target}")).map_err(|_| "login: the browser sent nothing usable")?.query_pairs().into_owned().collect();
    if let Some(error) = query.get("error") {
        return Err(format!("login: Dropbox said {error}"));
    }
    if query.get("state").map(String::as_str) != Some(state) {
        return Err("login: the answer is not for this login".into());
    }
    let code = query.get("code").cloned().ok_or("login: the browser sent no code")?;
    respond(stream, "Connected — you can close this tab.");
    Ok(code)
}

fn respond(stream: &std::net::TcpStream, text: &str) {
    let body = format!("<html><body><p>{text}</p></body></html>");
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Trades an auth code (or a refresh token) for tokens.
fn exchange(app_key: &str, code: &str, verifier: &str) -> Result<(String, String), String> {
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", code)
        .append_pair("grant_type", "authorization_code")
        .append_pair("client_id", app_key)
        .append_pair("code_verifier", verifier)
        .append_pair("redirect_uri", REDIRECT_URI)
        .finish();
    let reply = net::post(
        "https://api.dropboxapi.com/oauth2/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Dropbox login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    tokens(&reply.body)
}

/// Trades the kept refresh token for an access token.
fn access_token() -> Result<String, String> {
    let refresh = Active::password(TOKEN_ID).ok_or_else(|| "Dropbox is not connected.".to_string())?;
    let app_key = app_key()?;
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", &refresh)
        .append_pair("client_id", app_key)
        .finish();
    let reply = net::post(
        "https://api.dropboxapi.com/oauth2/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        body.as_bytes(),
    )?;
    if reply.status != 200 {
        return Err(format!("Dropbox login failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)));
    }
    tokens(&reply.body).map(|(access, _)| access)
}

fn tokens(body: &[u8]) -> Result<(String, String), String> {
    let tokens: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("login: {e}"))?;
    let access = tokens["access_token"].as_str().ok_or("login: Dropbox gave no access token")?.to_string();
    let refresh = tokens["refresh_token"].as_str().ok_or("login: Dropbox gave no refresh token (ask for offline access again)")?.to_string();
    Ok((access, refresh))
}

/// Downloads the positions file; `None` when there is none yet.
fn download(access: &str) -> Result<Option<HashMap<String, RemoteRecord>>, String> {
    let arg = serde_json::json!({ "path": FILE_PATH }).to_string();
    let reply = net::post(
        "https://content.dropboxapi.com/2/files/download",
        &[("Authorization", &format!("Bearer {access}")), ("Dropbox-API-Arg", &arg)],
        &[],
    )?;
    if reply.status == 200 {
        let file: SyncFile = serde_json::from_slice(&reply.body).map_err(|e| format!("Dropbox file is not ours: {e}"))?;
        return Ok(Some(file.positions));
    }
    if reply.status == 409 && String::from_utf8_lossy(&reply.body).contains("not_found") {
        return Ok(None);
    }
    Err(format!("Dropbox download failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)))
}

/// Uploads the merged positions file.
fn upload(access: &str, positions: &HashMap<String, RemoteRecord>) -> Result<(), String> {
    let file = SyncFile { version: FILE_VERSION, positions: positions.clone() };
    let body = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
    let arg = serde_json::json!({ "path": FILE_PATH, "mode": "overwrite", "autorename": false, "mute": true }).to_string();
    let reply = net::post(
        "https://content.dropboxapi.com/2/files/upload",
        &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/octet-stream"), ("Dropbox-API-Arg", &arg)],
        &body,
    )?;
    if reply.status == 200 {
        Ok(())
    } else {
        Err(format!("Dropbox upload failed ({}): {}", reply.status, String::from_utf8_lossy(&reply.body)))
    }
}

/// The open book's remote position, if any: a dead transport reports
/// nothing instead of failing the pull.
pub fn pull(fingerprint: &str) -> Option<kosync::Progress> {
    let access = access_token().ok()?;
    // `time` is ms here like locally; KOSync counts seconds.
    download(&access).ok()??.get(fingerprint).map(|r| kosync::Progress {
        progress: r.cfi.clone(),
        percentage: r.fraction,
        device: r.device.clone(),
        device_id: r.device_id.clone(),
        timestamp: r.time / 1000,
    })
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
    let access = access_token()?;
    let remote = download(&access)?.unwrap_or_default();
    let mut local = local.clone();
    local.insert(
        fingerprint.into(),
        crate::store::Position { cfi: cfi.into(), fraction, time: crate::clock::now_ms() },
    );
    let merged = merge_for_upload(&local, &remote, device, device_id);
    if !should_upload(&merged, &remote) {
        return Ok(());
    }
    upload(&access, &merged)
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
    fn pkce_challenge_matches_the_rfc_vector() {
        // RFC 7636, appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn auth_url_asks_for_a_code_with_pkce() {
        let url = auth_url("key", "challenge", "state");
        assert!(url.starts_with("https://www.dropbox.com/oauth2/authorize?"));
        assert!(url.contains("client_id=key"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        // The exact registered callback (the App Console requires a match).
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A51943%2Fdropbox"));
        assert!(url.contains("state=state"));
        assert!(url.contains("token_access_type=offline"));
    }

    #[test]
    fn app_key_is_configured() {
        // The App Console key; without it every Dropbox action refuses with help.
        assert!(!APP_KEY.is_empty());
        assert!(auth_url(APP_KEY, "challenge", "state").contains(&format!("client_id={APP_KEY}")));
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
}
