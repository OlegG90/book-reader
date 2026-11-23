//! Check for updates: asks GitHub for the repository's latest release, only
//! when the reader asks (Settings → About). Nothing is downloaded or
//! installed; the release page is only opened in the browser.

use crate::net::{self, Auth};
use serde::{Deserialize, Serialize};

/// Where Book Reader's source lives (Cargo.toml's `repository`), fixed at
/// build time so the page never names an address of its own.
pub const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// What the check found.
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// The latest release's version, without the tag's `v`.
    pub latest: String,
    /// True when it is newer than this copy.
    pub newer: bool,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// Asks GitHub's API for the latest release and compares it with this copy.
pub fn check() -> Result<UpdateCheck, String> {
    let path = REPOSITORY.trim_start_matches("https://github.com/").trim_end_matches('/');
    let url = format!("https://api.github.com/repos/{path}/releases/latest");
    let reply = net::get(&url, None, Auth::OnChallenge, "application/vnd.github+json")
        .map_err(|_| "Cannot reach GitHub: check the connection".to_string())?;
    match reply.status {
        200 => {}
        404 => return Err("There is no release yet".into()),
        403 | 429 => return Err("GitHub is not answering just now: try again later".into()),
        code => return Err(format!("GitHub answered {code}: try again later")),
    }
    let release: Release =
        serde_json::from_slice(&reply.body).map_err(|_| "GitHub's answer could not be read".to_string())?;
    let latest = release.tag_name.trim_start_matches('v').to_string();
    Ok(UpdateCheck { newer: is_newer(&latest, env!("CARGO_PKG_VERSION")), latest })
}

/// `latest` is a later version than `current`, both `X.Y.Z` (a missing part
/// counts as 0; a part that is not a number makes the version unreadable, so
/// never newer).
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parts(latest), parts(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

/// The three numbers of a version, or None when it can't be read.
fn parts(version: &str) -> Option<[u64; 3]> {
    let mut parts = [0; 3];
    for (i, part) in version.trim().trim_start_matches('v').split('.').enumerate() {
        *parts.get_mut(i)? = part.parse().ok()?;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_version_is_newer() {
        assert!(is_newer("0.8.0", "0.7.0"));
        assert!(is_newer("1.0.0", "0.7.9"));
        assert!(is_newer("0.7.10", "0.7.9"));
        assert!(is_newer("v0.7.1", "0.7.0"));
    }

    #[test]
    fn the_same_or_an_older_version_is_not() {
        assert!(!is_newer("0.7.0", "0.7.0"));
        assert!(!is_newer("0.6.9", "0.7.0"));
        assert!(!is_newer("0.7", "0.7.0"));
    }

    #[test]
    fn an_unreadable_version_is_never_newer() {
        assert!(!is_newer("0.8.0-beta", "0.7.0"));
        assert!(!is_newer("latest", "0.7.0"));
        assert!(!is_newer("1.2.3.4", "0.7.0"));
    }

    #[test]
    fn the_repository_is_on_github() {
        assert!(REPOSITORY.starts_with("https://github.com/"));
    }
}
