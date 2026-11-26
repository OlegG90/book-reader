//! Google Drive as a cloud store: OAuth (PKCE; loopback on the desktop, the
//! app's scheme on the phone) and the files of the app's hidden Drive folder
//! (`appDataFolder`; see cloud.rs for what they hold).
//!
//! The narrow `drive.appdata` scope keeps the rest of the Drive out of
//! reach. Secrets stay out of the config: the refresh token is kept like
//! catalog passwords (the OS credential store, the Android Keystore).

use crate::cloud::{self, access_with_life, token_post, tokens, CloudStore, ErrorSlot, TokenCache};
use crate::sync_error::SyncError;
use crate::platform::{Active, Platform};
use crate::{net, oauth};
use oauth::PendingLogin;

/// Google Drive, for the sync code.
pub struct Google {
    errors: ErrorSlot,
    tokens: TokenCache,
}

/// The one Google Drive store.
pub static STORE: Google = Google { errors: ErrorSlot::new(), tokens: TokenCache::new() };

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
pub(crate) const TOKEN_ID: &str = "google";

/// Google's OAuth token endpoint.
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

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
    cloud::keep_login(&STORE, &refresh)
}

/// Where the phone keeps a login waiting for the browser (its state and
/// PKCE verifier), in the credential store: Android may end the app while
/// the browser is open, and the answer must still find its login.
pub(crate) const LOGIN_ID: &str = "google-login";

/// How long a phone login waits for the browser's answer.
const LOGIN_WAIT_MS: u64 = 15 * 60 * 1000;

/// A waiting login as kept: state, verifier and start time, space-separated
/// (the first two are alphanumeric).
fn pending_login(state: &str, verifier: &str, started: u64) -> String {
    format!("{state} {verifier} {started}")
}

/// The state and verifier of a kept login, unless it is malformed or older
/// than `LOGIN_WAIT_MS`.
fn read_pending_login(kept: &str, now: u64) -> Result<(String, String), String> {
    let mut parts = kept.split(' ');
    let (Some(state), Some(verifier), Some(started)) = (parts.next(), parts.next(), parts.next()) else {
        return Err("login: no Google login is waiting".into());
    };
    let started: u64 = started.parse().map_err(|_| "login: no Google login is waiting")?;
    if now.saturating_sub(started) > LOGIN_WAIT_MS {
        return Err("login: the Google login took too long; connect again".into());
    }
    Ok((state.into(), verifier.into()))
}

/// Starts the phone login: open the returned address in the browser, and the
/// answer comes back to `finish_answer` through the app's intent filter.
pub fn begin_android() -> Result<String, String> {
    let (client_id, _) = credentials()?;
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    let url = auth_url(client_id, &oauth::challenge(&verifier), &state);
    Active::save_password(LOGIN_ID, &pending_login(&state, &verifier, crate::clock::now_ms()))?;
    Ok(url)
}

/// Finishes the phone login with the address the browser came back to; the
/// waiting login is used once, even when it fails.
pub fn finish_answer(answer: &str) -> Result<(), String> {
    let kept = Active::password(LOGIN_ID).ok_or("login: no Google login is waiting")?;
    Active::forget_password(LOGIN_ID);
    let (state, verifier) = read_pending_login(&kept, crate::clock::now_ms())?;
    let code = oauth::code_in_answer(answer, &state, "Google")?;
    let (_access, refresh) = exchange(&code, &verifier, Active::password(TOKEN_ID))?;
    cloud::keep_login(&STORE, &refresh)
}

/// Forgets the login, and a phone login still waiting for the browser.
pub fn disconnect() {
    cloud::disconnect(&STORE);
    Active::forget_password(LOGIN_ID);
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
    tokens(&token_post(TOKEN_URL, &form.finish(), "Google")?, stored, "Google")
}

/// A file's id in the app folder by its name, if there is one. Drive allows
/// two files with one name (two devices creating it at once); every device
/// then picks the oldest, so they all keep reading and writing the same one.
fn find_file(access: &str, name: &str) -> Result<Option<String>, SyncError> {
    Ok(find_files(access, name, 1)?.into_iter().next())
}

/// The ids of the app folder's files with this name, oldest first, at most `limit`.
fn find_files(access: &str, name: &str, limit: u32) -> Result<Vec<String>, SyncError> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("spaces", "appDataFolder")
        .append_pair("q", &name_query(name))
        .append_pair("orderBy", "createdTime")
        .append_pair("fields", "files(id)")
        .append_pair("pageSize", &limit.to_string())
        .finish();
    let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files?{query}"), access).map_err(SyncError::offline)?;
    if reply.status != 200 {
        return Err(SyncError::from_reply("Google Drive list", &reply));
    }
    let list: serde_json::Value =
        serde_json::from_slice(&reply.body).map_err(|e| SyncError::data(format!("Google Drive list is not ours: {e}")))?;
    Ok(list["files"].as_array().into_iter().flatten().filter_map(|file| file["id"].as_str()).map(str::to_string).collect())
}

/// The Drive search for one file name; quotes and backslashes in book names
/// are escaped, as the query language asks.
fn name_query(name: &str) -> String {
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    format!("name = '{escaped}' and trashed = false")
}

/// Downloads an opaque cloud file by its id.
fn download_bytes(access: &str, file_id: &str) -> Result<Vec<u8>, SyncError> {
    let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files/{file_id}?alt=media"), access).map_err(SyncError::offline)?;
    if reply.status != 200 {
        return Err(SyncError::from_reply("Google Drive download", &reply));
    }
    Ok(reply.body)
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
) -> Result<(), SyncError> {
    let (content_type, body) = multipart_body(metadata, data, content_type).map_err(SyncError::data)?;
    let url = match file_id {
        Some(id) => format!("https://www.googleapis.com/upload/drive/v3/files/{id}?uploadType=multipart"),
        None => "https://www.googleapis.com/upload/drive/v3/files?uploadType=multipart".into(),
    };
    let headers = [("Authorization", format!("Bearer {access}")), ("Content-Type", content_type)];
    let headers: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, value.as_str())).collect();
    let reply = match file_id {
        Some(_) => net::patch(&url, &headers, &body),
        None => net::post(&url, &headers, &body),
    }
    .map_err(SyncError::offline)?;
    if reply.status == 200 {
        Ok(())
    } else {
        Err(SyncError::from_reply("Google Drive upload", &reply))
    }
}

/// Uploads an opaque cloud file: created in the app folder the first time,
/// replaced afterwards.
fn upload_bytes(access: &str, file_id: Option<&str>, name: &str, data: &[u8]) -> Result<(), SyncError> {
    let metadata = match file_id {
        Some(_) => serde_json::json!({}),
        None => serde_json::json!({ "name": name, "parents": ["appDataFolder"] }),
    };
    // The positions file and the manifest are JSON; book files are opaque.
    let content_type = if name.ends_with(".json") { "application/json" } else { "application/octet-stream" };
    store_bytes(access, file_id, metadata, data.to_vec(), content_type)
}

impl CloudStore for Google {
    fn id(&self) -> &'static str {
        "google"
    }

    fn module(&self) -> &'static str {
        crate::modules::id::GOOGLE_DRIVE
    }

    fn token_id(&self) -> &'static str {
        TOKEN_ID
    }

    fn label(&self) -> &'static str {
        "Google Drive"
    }

    fn refresh(&self, refresh_token: &str) -> Result<(String, u64), SyncError> {
        let (client_id, client_secret) = credentials()?;
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("refresh_token", refresh_token)
            .append_pair("client_id", client_id)
            .append_pair("grant_type", "refresh_token");
        if !client_secret.is_empty() {
            form.append_pair("client_secret", client_secret);
        }
        // A refresh answer carries no refresh token (only the code exchange does).
        access_with_life(&token_post(TOKEN_URL, &form.finish(), "Google")?, "Google").map_err(SyncError::data)
    }

    fn get(&self, access: &str, name: &str) -> Result<Option<Vec<u8>>, SyncError> {
        let Some(id) = find_file(access, name)? else { return Ok(None) };
        download_bytes(access, &id).map(Some)
    }

    fn put(&self, access: &str, name: &str, bytes: &[u8]) -> Result<(), SyncError> {
        let file_id = find_file(access, name)?;
        upload_bytes(access, file_id.as_deref(), name, bytes)
    }

    fn delete(&self, access: &str, name: &str) -> Result<(), SyncError> {
        // Two files may share a name (see find_file): every one goes, each
        // looked up once (a listing that lags must not loop).
        for id in find_files(access, name, 10)? {
            let reply = net::bearer_delete(&format!("https://www.googleapis.com/drive/v3/files/{id}"), access).map_err(SyncError::offline)?;
            if reply.status != 204 && reply.status != 200 && reply.status != 404 {
                return Err(SyncError::from_reply("Google Drive delete", &reply));
            }
        }
        Ok(())
    }

    fn list(&self, access: &str, prefix: &str) -> Result<Vec<String>, SyncError> {
        let mut names = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("spaces", "appDataFolder")
                // The whole app folder, filtered here: how Drive's "contains"
                // splits a name with dashes into words isn't documented.
                .append_pair("q", "trashed = false")
                .append_pair("fields", "nextPageToken,files(name)")
                .append_pair("pageSize", "1000");
            if let Some(token) = &page_token {
                query.append_pair("pageToken", token);
            }
            let reply = net::bearer_get(&format!("https://www.googleapis.com/drive/v3/files?{}", query.finish()), access).map_err(SyncError::offline)?;
            if reply.status != 200 {
                return Err(SyncError::from_reply("Google Drive list", &reply));
            }
            let page: serde_json::Value =
                serde_json::from_slice(&reply.body).map_err(|e| SyncError::data(format!("Google Drive list is not ours: {e}")))?;
            for file in page["files"].as_array().into_iter().flatten() {
                if let Some(name) = file["name"].as_str().filter(|n| n.starts_with(prefix)) {
                    names.push(name.to_string());
                }
            }
            match page["nextPageToken"].as_str() {
                Some(token) => page_token = Some(token.to_string()),
                None => return Ok(names),
            }
        }
    }

    fn errors(&self) -> &ErrorSlot {
        &self.errors
    }

    fn tokens(&self) -> &TokenCache {
        &self.tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::{access_from, pick_refresh, token_json};

    #[test]
    fn a_kept_phone_login_survives_until_it_is_too_old() {
        let kept = pending_login("st", "ver", 1_000);
        assert_eq!(read_pending_login(&kept, 1_000 + 60_000).unwrap(), ("st".to_string(), "ver".to_string()));
        assert!(read_pending_login(&kept, 1_000 + LOGIN_WAIT_MS + 1).is_err());
        assert!(read_pending_login("garbage", 1_000).is_err());
    }

    #[test]
    fn names_with_quotes_stay_one_search_term() {
        assert_eq!(name_query("sync-books.json"), "name = 'sync-books.json' and trashed = false");
        assert_eq!(name_query("book-fp-O'Brien.epub"), r"name = 'book-fp-O\'Brien.epub' and trashed = false");
        assert_eq!(name_query(r"a\b"), r"name = 'a\\b' and trashed = false");
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
