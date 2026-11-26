//! Dropbox as a cloud store: OAuth (PKCE over a loopback address) and the
//! files of the app's own Dropbox folder (see cloud.rs for what they hold).
//!
//! The app never sees the rest of the Dropbox. Secrets stay out of the
//! config: the refresh token is kept like catalog passwords (the OS
//! credential store, the Android Keystore).

use crate::cloud::{self, access_with_life, token_post, tokens, CloudStore, ErrorSlot, TokenCache};
use crate::sync_error::SyncError;
use crate::platform::{Active, Platform};
use crate::{net, oauth};
use oauth::PendingLogin;

/// Dropbox, for the sync code.
pub struct Dropbox {
    errors: ErrorSlot,
    tokens: TokenCache,
}

/// The one Dropbox store.
pub static STORE: Dropbox = Dropbox { errors: ErrorSlot::new(), tokens: TokenCache::new() };

/// The Dropbox app key: the "BookReader Sync" scoped app in the App Console
/// with app-folder access (files.content.read + files.content.write).
pub const APP_KEY: &str = "hq54wf951whc1jv";

/// Where the refresh token is kept, like catalog passwords.
pub(crate) const TOKEN_ID: &str = "dropbox";

/// Dropbox's OAuth token endpoint.
const TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";

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

/// Listens on the loopback addresses and builds the Dropbox authorize
/// address. The port is fixed: it must match the URI registered in the App
/// Console.
pub fn begin() -> Result<PendingLogin, String> {
    let app_key = app_key()?;
    let verifier = oauth::random_string(64);
    let state = oauth::random_string(16);
    let listener = oauth::bind(51943)?;
    let url = auth_url(app_key, &oauth::challenge(&verifier), &state);
    Ok(PendingLogin { verifier, state, listener, url })
}

/// Waits for the browser's code (or the timeout), trades it for tokens and
/// keeps the refresh token like a catalog password.
pub fn finish(login: PendingLogin) -> Result<(), String> {
    let code = oauth::wait_for_code(login.listener, &login.state, "Dropbox")?;
    let (_access, refresh) = exchange(app_key()?, &code, &login.verifier, Active::password(TOKEN_ID))?;
    cloud::keep_login(&STORE, &refresh)
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
    tokens(&token_post(TOKEN_URL, &body, "Dropbox")?, stored, "Dropbox")
}

impl CloudStore for Dropbox {
    fn id(&self) -> &'static str {
        "dropbox"
    }

    fn module(&self) -> &'static str {
        crate::modules::id::DROPBOX
    }

    fn token_id(&self) -> &'static str {
        TOKEN_ID
    }

    fn label(&self) -> &'static str {
        "Dropbox"
    }

    fn refresh(&self, refresh_token: &str) -> Result<(String, u64), SyncError> {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "refresh_token")
            .append_pair("refresh_token", refresh_token)
            .append_pair("client_id", app_key()?)
            .finish();
        // A refresh answer carries no refresh token (only the code exchange does).
        access_with_life(&token_post(TOKEN_URL, &body, "Dropbox")?, "Dropbox").map_err(SyncError::data)
    }

    fn get(&self, access: &str, name: &str) -> Result<Option<Vec<u8>>, SyncError> {
        let arg = serde_json::json!({ "path": format!("/{name}") }).to_string();
        let reply = net::post(
            "https://content.dropboxapi.com/2/files/download",
            &[("Authorization", &format!("Bearer {access}")), ("Dropbox-API-Arg", &arg)],
            &[],
        )
        .map_err(SyncError::offline)?;
        if reply.status == 200 {
            return Ok(Some(reply.body));
        }
        if reply.status == 409 && String::from_utf8_lossy(&reply.body).contains("not_found") {
            return Ok(None);
        }
        Err(SyncError::from_reply("Dropbox download", &reply))
    }

    fn put(&self, access: &str, name: &str, bytes: &[u8]) -> Result<(), SyncError> {
        let arg = serde_json::json!({ "path": format!("/{name}"), "mode": "overwrite", "autorename": false, "mute": true }).to_string();
        let reply = net::post(
            "https://content.dropboxapi.com/2/files/upload",
            &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/octet-stream"), ("Dropbox-API-Arg", &arg)],
            bytes,
        )
        .map_err(SyncError::offline)?;
        if reply.status == 200 {
            Ok(())
        } else {
            Err(SyncError::from_reply("Dropbox upload", &reply))
        }
    }

    fn delete(&self, access: &str, name: &str) -> Result<(), SyncError> {
        let body = serde_json::json!({ "path": format!("/{name}") }).to_string();
        let reply = net::post(
            "https://api.dropboxapi.com/2/files/delete_v2",
            &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/json")],
            body.as_bytes(),
        )
        .map_err(SyncError::offline)?;
        if reply.status == 200 || (reply.status == 409 && String::from_utf8_lossy(&reply.body).contains("not_found")) {
            Ok(())
        } else {
            Err(SyncError::from_reply("Dropbox delete", &reply))
        }
    }

    fn space(&self, access: &str) -> Result<Option<(u64, u64)>, SyncError> {
        let reply = net::post(
            "https://api.dropboxapi.com/2/users/get_space_usage",
            &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/json")],
            b"null",
        )
        .map_err(SyncError::offline)?;
        if reply.status != 200 {
            return Err(SyncError::from_reply("Dropbox space", &reply));
        }
        let usage: serde_json::Value = serde_json::from_slice(&reply.body).map_err(|e| SyncError::data(format!("Dropbox space is not ours: {e}")))?;
        // A team account shares its space: its own `used` counts against the team's.
        let used = usage["allocation"]["used"].as_u64().or_else(|| usage["used"].as_u64());
        Ok(used.zip(usage["allocation"]["allocated"].as_u64()))
    }

    fn list(&self, access: &str, prefix: &str) -> Result<Vec<String>, SyncError> {
        let mut names = Vec::new();
        let mut url = "https://api.dropboxapi.com/2/files/list_folder";
        let mut body = serde_json::json!({ "path": "" });
        loop {
            let reply = net::post(
                url,
                &[("Authorization", &format!("Bearer {access}")), ("Content-Type", "application/json")],
                body.to_string().as_bytes(),
            )
            .map_err(SyncError::offline)?;
            // An app folder nothing was ever written to doesn't exist yet.
            if reply.status == 409 && String::from_utf8_lossy(&reply.body).contains("not_found") {
                return Ok(names);
            }
            if reply.status != 200 {
                return Err(SyncError::from_reply("Dropbox list", &reply));
            }
            let page: serde_json::Value =
                serde_json::from_slice(&reply.body).map_err(|e| SyncError::data(format!("Dropbox list is not ours: {e}")))?;
            for entry in page["entries"].as_array().into_iter().flatten() {
                if let Some(name) = entry["name"].as_str().filter(|n| entry[".tag"] == "file" && n.starts_with(prefix)) {
                    names.push(name.to_string());
                }
            }
            if page["has_more"] != true {
                return Ok(names);
            }
            // The rest of a long folder (book files share it) comes page by page.
            url = "https://api.dropboxapi.com/2/files/list_folder/continue";
            body = serde_json::json!({ "cursor": page["cursor"] });
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
    use crate::oauth::challenge;

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

}
