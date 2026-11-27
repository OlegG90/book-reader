//! OneDrive as a cloud store: OAuth (PKCE over a loopback address, through
//! the Microsoft identity platform) and the files of the app's own OneDrive
//! folder (`Apps/Book Reader`; see cloud.rs for what they hold).
//!
//! The narrow `Files.ReadWrite.AppFolder` scope keeps the rest of the
//! OneDrive out of reach. Secrets stay out of the config: the refresh token
//! is kept like catalog passwords (the OS credential store, the Android
//! Keystore).

use crate::cloud::{self, access_with_life, code_post, token_json, token_post, tokens, CloudStore, ErrorSlot, TokenCache};
use crate::sync_error::SyncError;
use crate::platform::{Active, Platform};
use crate::{net, oauth};
use oauth::PendingLogin;

/// OneDrive, for the sync code.
pub struct OneDrive {
    errors: ErrorSlot,
    tokens: TokenCache,
}

/// The one OneDrive store.
pub static STORE: OneDrive = OneDrive { errors: ErrorSlot::new(), tokens: TokenCache::new() };

/// The application (client) ID of the "Book Reader" registration in
/// Microsoft Entra: personal Microsoft accounts only, a public client with
/// no secret. Like the Dropbox app key, a public identifier, not a secret.
pub const CLIENT_ID: &str = "49f35fef-3df6-4d60-a932-cd6865fd6788";

/// Where the refresh token is kept, like catalog passwords.
pub(crate) const TOKEN_ID: &str = "onedrive";

/// The Microsoft identity platform for personal accounts: where the
/// browser signs in, and where codes and refresh tokens become tokens.
const AUTHORIZE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";

/// The app's own folder, and a refresh token to stay signed in.
const SCOPE: &str = "Files.ReadWrite.AppFolder offline_access";

/// The OAuth callback, registered under "Mobile and desktop applications".
/// No manifest or key-hash changes are needed: the loopback address works
/// in the desktop browser and on the phone. The login listens on `PORT`.
const REDIRECT_URI: &str = "http://localhost:51945/onedrive";
const PORT: u16 = 51945;

/// The app folder in Microsoft Graph.
const APP_ROOT: &str = "https://graph.microsoft.com/v1.0/me/drive/special/approot";

/// The OneDrive authorize address for the loopback flow.
fn auth_url(challenge: &str, state: &str) -> String {
    url::form_urlencoded::Serializer::new(format!("{AUTHORIZE_URL}?"))
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .finish()
}

/// Listens on the loopback addresses and builds the OneDrive authorize
/// address. The port is fixed: it is part of the registered redirect URI.
pub fn begin() -> Result<PendingLogin, String> {
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    let listener = oauth::bind(PORT)?;
    let url = auth_url(&oauth::challenge(&verifier), &state);
    Ok(PendingLogin { verifier, state, listener, url })
}

/// Waits for the browser's code (or the timeout), trades it for tokens and
/// keeps the refresh token like a catalog password.
pub fn finish(login: PendingLogin) -> Result<(), String> {
    let code = oauth::wait_for_code(login.listener, &login.state, STORE.label())?;
    let form = token_form(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT_URI),
        ("code_verifier", &login.verifier),
    ]);
    let (_access, refresh) = tokens(&code_post(TOKEN_URL, &form, STORE.label())?, Active::password(TOKEN_ID), STORE.label())?;
    cloud::keep_login(&STORE, &refresh)
}

/// The form for a token request: the grant's own fields, plus the client
/// and the scope every request names.
fn token_form(grant: &[(&str, &str)]) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("client_id", CLIENT_ID).extend_pairs(grant).append_pair("scope", SCOPE);
    form.finish()
}

/// The new refresh token a refresh answer carries, if any. Microsoft hands
/// out a new one with every refresh, and the old one is not extended.
fn rotated_refresh(body: &[u8]) -> Option<String> {
    token_json(body).ok()?["refresh_token"].as_str().map(str::to_string)
}

/// A Graph answer's JSON, or a sync failure naming what was being read.
fn graph_json(body: &[u8], what: &str) -> Result<serde_json::Value, SyncError> {
    serde_json::from_slice(body).map_err(|e| SyncError::data(format!("OneDrive {what} is not ours: {e}")))
}

/// A file's name in the app folder. OneDrive refuses some characters that
/// book file names may have (`"*:<>?|`); they become `_`, the same way
/// every time, so a file is always found under the name it was written as.
fn drive_name(name: &str) -> String {
    name.chars().map(|c| if "\"*:<>?|".contains(c) { '_' } else { c }).collect()
}

/// The Graph address of a file in the app folder, plus `rest` (":/content"
/// for its bytes, "" for its metadata). The name is percent-encoded as one
/// path segment.
fn item_url(name: &str, rest: &str) -> String {
    let mut url = url::Url::parse(&format!("{APP_ROOT}:")).expect("the app folder address is valid");
    url.path_segments_mut().expect("an https address has a path").push(&drive_name(name));
    format!("{url}{rest}")
}

impl CloudStore for OneDrive {
    fn id(&self) -> &'static str {
        "onedrive"
    }

    fn module(&self) -> &'static str {
        crate::modules::id::ONEDRIVE
    }

    fn token_id(&self) -> &'static str {
        TOKEN_ID
    }

    fn label(&self) -> &'static str {
        "OneDrive"
    }

    fn refresh(&self, refresh_token: &str) -> Result<(String, u64), SyncError> {
        let form = token_form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token)]);
        let answer = token_post(TOKEN_URL, &form, self.label())?;
        let (access, lives) = access_with_life(&answer, self.label()).map_err(SyncError::data)?;
        // Keep the newest refresh token, or the login runs out in 90 days.
        // A failed save still leaves the kept one, which stays valid: the
        // access token is good either way.
        if let Some(fresh) = rotated_refresh(&answer).filter(|f| f != refresh_token) {
            let _ = Active::save_password(self.token_id(), &fresh);
        }
        Ok((access, lives))
    }

    fn get(&self, access: &str, name: &str) -> Result<Option<Vec<u8>>, SyncError> {
        // The bytes live on another server: the file's metadata names a
        // short-lived address that needs no token.
        let reply = net::bearer_get(&item_url(name, ""), access).map_err(SyncError::offline)?;
        if reply.status == 404 {
            return Ok(None);
        }
        if reply.status != 200 {
            return Err(SyncError::from_reply("OneDrive download", &reply));
        }
        let item = graph_json(&reply.body, "file")?;
        let address = item["@microsoft.graph.downloadUrl"].as_str().ok_or_else(|| SyncError::data("OneDrive gave no download address"))?;
        let reply = net::get(address, None, net::Auth::OnChallenge, "*/*").map_err(SyncError::offline)?;
        if reply.status != 200 {
            return Err(SyncError::from_reply("OneDrive download", &reply));
        }
        Ok(Some(reply.body))
    }

    fn put(&self, access: &str, name: &str, bytes: &[u8]) -> Result<(), SyncError> {
        let reply = net::put(
            &item_url(name, ":/content?@microsoft.graph.conflictBehavior=replace"),
            &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/octet-stream")],
            bytes,
        )
        .map_err(SyncError::offline)?;
        if matches!(reply.status, 200 | 201) {
            Ok(())
        } else {
            Err(SyncError::from_reply("OneDrive upload", &reply))
        }
    }

    fn delete(&self, access: &str, name: &str) -> Result<(), SyncError> {
        let reply = net::bearer_delete(&item_url(name, ""), access).map_err(SyncError::offline)?;
        if matches!(reply.status, 200 | 204 | 404) {
            Ok(())
        } else {
            Err(SyncError::from_reply("OneDrive delete", &reply))
        }
    }

    fn space(&self, access: &str) -> Result<Option<(u64, u64)>, SyncError> {
        let reply = net::bearer_get("https://graph.microsoft.com/v1.0/me/drive?$select=quota", access).map_err(SyncError::offline)?;
        if reply.status != 200 {
            return Err(SyncError::from_reply("OneDrive space", &reply));
        }
        let drive = graph_json(&reply.body, "space")?;
        Ok(drive["quota"]["used"].as_u64().zip(drive["quota"]["total"].as_u64()))
    }

    fn list(&self, access: &str, prefix: &str) -> Result<Vec<String>, SyncError> {
        let mut names = Vec::new();
        let mut url = format!("{APP_ROOT}/children?$select=name,file&$top=1000");
        loop {
            let reply = net::bearer_get(&url, access).map_err(SyncError::offline)?;
            // An app folder nothing was ever written to may not exist yet.
            if reply.status == 404 {
                return Ok(names);
            }
            if reply.status != 200 {
                return Err(SyncError::from_reply("OneDrive list", &reply));
            }
            let page = graph_json(&reply.body, "list")?;
            for item in page["value"].as_array().into_iter().flatten() {
                if let Some(name) = item["name"].as_str().filter(|n| item.get("file").is_some() && n.starts_with(prefix)) {
                    names.push(name.to_string());
                }
            }
            // The rest of a long folder (book files share it) comes page by page.
            match page["@odata.nextLink"].as_str() {
                Some(next) => url = next.to_string(),
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

    #[test]
    fn auth_url_asks_personal_accounts_for_a_code_with_pkce() {
        let url = auth_url("challenge", "state");
        assert!(url.starts_with("https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize?"));
        assert!(url.contains(&format!("client_id={CLIENT_ID}")));
        // The exact registered callback.
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A51945%2Fonedrive"));
        assert!(url.contains("scope=Files.ReadWrite.AppFolder+offline_access"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state"));
    }

    #[test]
    fn the_login_listens_where_the_redirect_points() {
        assert_eq!(REDIRECT_URI, format!("http://localhost:{PORT}/onedrive"));
    }

    #[test]
    fn token_requests_name_the_client_and_the_scope() {
        let form = token_form(&[("grant_type", "refresh_token"), ("refresh_token", "M.C5")]);
        assert_eq!(form, format!("client_id={CLIENT_ID}&grant_type=refresh_token&refresh_token=M.C5&scope=Files.ReadWrite.AppFolder+offline_access"));
    }

    #[test]
    fn a_refresh_answer_may_carry_a_new_refresh_token() {
        let rotated = br#"{"token_type": "Bearer", "expires_in": 3600, "access_token": "EwB", "refresh_token": "M.C5"}"#;
        assert_eq!(access_with_life(rotated, "OneDrive").unwrap(), ("EwB".into(), 3600));
        assert_eq!(rotated_refresh(rotated).as_deref(), Some("M.C5"));
        assert_eq!(rotated_refresh(br#"{"access_token": "EwB"}"#), None);
    }

    #[test]
    fn file_addresses_are_one_safe_segment_in_the_app_folder() {
        let root = "https://graph.microsoft.com/v1.0/me/drive/special/approot:";
        assert_eq!(item_url("positions-pc.json", ":/content"), format!("{root}/positions-pc.json:/content"));
        assert_eq!(item_url("book-fp-A b#1%.epub", ""), format!("{root}/book-fp-A%20b%231%25.epub"));
        // Characters OneDrive refuses map the same way every time.
        assert_eq!(item_url("book-fp-Why? \"Now\": a|b.fb2", ""), format!("{root}/book-fp-Why_%20_Now__%20a_b.fb2"));
        assert_eq!(drive_name("Why?"), drive_name("Why?"));
    }
}
