//! Why a sync step failed, so each kind gets its own handling: a cloud
//! without a login is skipped instead of failing, a rejected login drops the
//! kept access token, the rest is reported and retried. The message is what
//! the settings and the logs show; the kind is for the code.

use std::fmt;

/// The kind of a sync failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// No login is kept: nothing to sync with, not a failure.
    NotConnected,
    /// The request never got an answer (no network, a timeout, a refused connection).
    Offline,
    /// The server turned the login down (an expired or revoked token).
    Rejected,
    /// The server asks to slow down. Shown like any failure for now; it is
    /// kept apart for a back-off in the sync engine (#101).
    RateLimited,
    /// Any other answer that isn't a success.
    Http,
    /// The answer came, but it isn't what the app wrote (a foreign or broken file).
    Data,
    /// A failure from code that doesn't sort its failures (KOSync, the
    /// configuration, the local store).
    Other,
}

/// A sync failure: its kind and the sentence shown for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncError {
    pub kind: Kind,
    pub message: String,
}

impl SyncError {
    /// A failure of `kind` saying `message`.
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        SyncError { kind, message: message.into() }
    }

    /// A request that got no answer; `message` (from net) says why.
    pub fn offline(message: String) -> Self {
        SyncError::new(Kind::Offline, message)
    }

    /// A file that isn't what the app wrote.
    pub fn data(message: impl Into<String>) -> Self {
        SyncError::new(Kind::Data, message)
    }

    /// An answer that isn't a success: "<what> failed (status): <what the
    /// server said>", sorted by status — 401 and 403 turn the login down, 429
    /// asks to slow down. Google Drive also slows clients down with a 403
    /// that names a rate limit, which counts as slowing down too.
    pub fn from_reply(what: &str, reply: &crate::net::Reply) -> Self {
        let said = String::from_utf8_lossy(&reply.body);
        let kind = match reply.status {
            429 => Kind::RateLimited,
            403 if said.contains("ateLimitExceeded") => Kind::RateLimited,
            401 | 403 => Kind::Rejected,
            _ => Kind::Http,
        };
        SyncError::new(kind, format!("{what} failed ({}): {said}", reply.status))
    }
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Commands report failures as sentences.
impl From<SyncError> for String {
    fn from(e: SyncError) -> String {
        e.message
    }
}

/// A sentence from code that doesn't sort its failures (KOSync, the
/// configuration, the local store).
impl From<String> for SyncError {
    fn from(message: String) -> Self {
        SyncError::new(Kind::Other, message)
    }
}

/// The same, for a fixed sentence.
impl From<&str> for SyncError {
    fn from(message: &str) -> Self {
        SyncError::new(Kind::Other, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16) -> crate::net::Reply {
        crate::net::Reply::for_test(status, b"nope")
    }

    #[test]
    fn drive_rate_limits_are_not_a_rejected_login() {
        let throttled = crate::net::Reply::for_test(403, br#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#);
        assert_eq!(SyncError::from_reply("Google Drive list", &throttled).kind, Kind::RateLimited);
        assert_eq!(SyncError::from("kosync: no answer".to_string()).kind, Kind::Other);
    }

    #[test]
    fn answers_are_sorted_by_status() {
        assert_eq!(SyncError::from_reply("Dropbox download", &reply(401)).kind, Kind::Rejected);
        assert_eq!(SyncError::from_reply("Dropbox download", &reply(403)).kind, Kind::Rejected);
        assert_eq!(SyncError::from_reply("Dropbox download", &reply(429)).kind, Kind::RateLimited);
        let e = SyncError::from_reply("Dropbox download", &reply(500));
        assert_eq!(e.kind, Kind::Http);
        assert_eq!(e.to_string(), "Dropbox download failed (500): nope");
        assert_eq!(String::from(e), "Dropbox download failed (500): nope");
    }
}
