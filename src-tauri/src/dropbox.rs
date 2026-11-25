//! Reading-progress sync through Dropbox: OAuth (PKCE over a loopback
//! address), one fingerprint-keyed positions file, newest-wins merge.
//!
//! The file lives in the app's own Dropbox folder (`/sync-state.json`), so
//! the app never sees the rest of the Dropbox. Secrets stay out of the
//! config: the refresh token is kept like catalog passwords (the OS
//! credential store, the Android Keystore).

use crate::cloud::{ErrorSlot, RemoteRecord, SyncFile, access_from, merge_for_upload, should_upload, token_json, tokens};
use crate::platform::{Active, Platform};
use crate::{kosync, net, oauth};
use oauth::PendingLogin;
use std::collections::HashMap;

/// The last Dropbox sync failure, for the settings.
static LAST_ERROR: ErrorSlot = ErrorSlot::new();

/// The last sync failure, if any.
pub fn last_error() -> Option<String> {
    LAST_ERROR.last()
}

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

/// The app key, or why Dropbox cannot work yet.
fn app_key() -> Result<&'static str, String> {
    if APP_KEY.is_empty() {
        return Err("Dropbox is not set up yet: create a scoped app (app-folder access) in the Dropbox App Console and put its key in dropbox::APP_KEY.".into());
    }
    Ok(APP_KEY)
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
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    // `localhost` may resolve to ::1 or 127.0.0.1 depending on the device;
    // listen on both where the stack allows it, so the browser always lands.
    let listener = oauth::bind(51943)?;
    let url = auth_url(app_key, &oauth::challenge(&verifier), &state);
    Ok(PendingLogin { verifier, state, listener, url })
}

/// Waits for the browser's code (or the timeout), trades it for tokens and
/// keeps the refresh token like a catalog password.
pub fn finish(login: PendingLogin) -> Result<(), String> {
    let code = oauth::wait_for_code(login.listener, &login.state, "Dropbox")?;
    let (_access, refresh) = exchange(app_key()?, &code, &login.verifier, Active::password(TOKEN_ID))?;
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

/// Trades an auth code (or a refresh token) for tokens.
fn exchange(app_key: &str, code: &str, verifier: &str, stored: Option<String>) -> Result<(String, String), String> {
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
    tokens(&reply.body, stored, "Dropbox")
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
    // A refresh answer carries no refresh token (only the code exchange does).
    access_from(&token_json(&reply.body)?, "Dropbox")
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
    LAST_ERROR.track(pull_inner(fingerprint)).ok().flatten()
}

/// The cloud's whole positions file, if the app ever synced.
pub fn remote_positions() -> Option<HashMap<String, RemoteRecord>> {
    LAST_ERROR.track(remote_map()).ok().flatten()
}

fn remote_map() -> Result<Option<HashMap<String, RemoteRecord>>, String> {
    let access = access_token()?;
    download(&access)
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
    use crate::cloud::pick_refresh;
    use crate::oauth::challenge;

    fn record(cfi: &str, time: u64) -> RemoteRecord {
        RemoteRecord { cfi: cfi.into(), fraction: 0.5, time, device: "phone".into(), device_id: "phone".into() }
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
    fn failures_are_remembered_until_a_success() {
        assert_eq!(last_error(), None);
        let _ = LAST_ERROR.track::<()>(Err("gone".into()));
        assert_eq!(last_error().as_deref(), Some("gone"));
        let _ = LAST_ERROR.track::<()>(Ok(()));
        assert_eq!(last_error(), None);
    }

    #[test]
    fn refresh_answer_carries_no_refresh_token() {
        // What /oauth2/token answers to a refresh grant: access only.
        let body = br#"{"access_token": "sl.XXXX", "token_type": "bearer", "expires_in": 14400}"#;
        assert_eq!(access_from(&token_json(body).unwrap(), "Dropbox").unwrap(), "sl.XXXX");
        assert!(tokens(body, None, "Dropbox").is_err(), "a code exchange without one still fails");
        assert_eq!(tokens(body, Some("old".into()), "Dropbox"), Ok(("sl.XXXX".into(), "old".into())));
    }

    #[test]
    fn reauth_without_a_new_token_keeps_the_kept_one() {
        assert_eq!(pick_refresh(Some("new".into()), Some("old".into()), "Dropbox"), Ok("new".into()));
        assert_eq!(pick_refresh(None, Some("old".into()), "Dropbox"), Ok("old".into()));
        assert!(pick_refresh(None, None, "Dropbox").is_err());
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
}
