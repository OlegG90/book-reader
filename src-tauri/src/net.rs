//! HTTP for the catalogs and sync: GET and PUT with the server's login,
//! HTTP Basic (Calibre-Web-Automated) or Digest (the Calibre Content Server).

use md5::{Digest, Md5};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use ureq::Agent;

/// A server login.
#[derive(Clone)]
pub struct Login {
    pub user: String,
    pub password: String,
}

/// What a request brought back.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The largest download accepted: a big illustrated book, with room to spare.
const MAX_BODY: u64 = 300 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

fn agent() -> Agent {
    Agent::config_builder()
        // 401 is an answer to act on (Digest), not an error.
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("BookReader/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// GET `url`, with the login if there is one.
pub fn get(url: &str, login: Option<&Login>, accept: &str) -> Result<Reply, String> {
    send("GET", url, login, accept, None)
}

/// PUT a JSON body to `url`.
pub fn put_json(url: &str, login: Option<&Login>, accept: &str, body: &str) -> Result<Reply, String> {
    send("PUT", url, login, accept, Some(body))
}

fn send(method: &str, url: &str, login: Option<&Login>, accept: &str, body: Option<&str>) -> Result<Reply, String> {
    let agent = agent();
    // Basic goes along with the first request (CWA expects it); a Digest
    // challenge in the answer gets a second request.
    let mut reply = request(&agent, method, url, accept, login.map(basic_header).as_deref(), body)?;
    if reply.status == 401 {
        if let (Some(login), Some(challenge)) = (login, reply.challenge.as_deref().and_then(DigestChallenge::parse)) {
            let uri = request_uri(url);
            let header = challenge.header(login, method, &uri, &new_cnonce());
            reply = request(&agent, method, url, accept, Some(&header), body)?;
        }
    }
    Ok(reply.reply)
}

struct Answer {
    reply: Reply,
    /// The `WWW-Authenticate` header, on a 401.
    challenge: Option<String>,
}

impl std::ops::Deref for Answer {
    type Target = Reply;
    fn deref(&self) -> &Reply {
        &self.reply
    }
}

fn request(agent: &Agent, method: &str, url: &str, accept: &str, auth: Option<&str>, body: Option<&str>) -> Result<Answer, String> {
    let failed = |e: ureq::Error| format!("{}: {e}", host_of(url));
    let response = match (method, body) {
        ("PUT", Some(body)) => {
            let mut req = agent.put(url).header("Accept", accept).header("Content-Type", "application/json");
            if let Some(auth) = auth {
                req = req.header("Authorization", auth);
            }
            req.send(body).map_err(failed)?
        }
        _ => {
            let mut req = agent.get(url).header("Accept", accept);
            if let Some(auth) = auth {
                req = req.header("Authorization", auth);
            }
            req.call().map_err(failed)?
        }
    };
    let status = response.status().as_u16();
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let challenge = header("www-authenticate");
    let mut response = response;
    let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(failed)?;
    Ok(Answer { reply: Reply { status, body }, challenge })
}

/// "host" of a URL, for messages.
fn host_of(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string())
}

/// The path and query a Digest response is computed over.
fn request_uri(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        },
        Err(_) => url.to_string(),
    }
}

fn basic_header(login: &Login) -> String {
    use base64::Engine;
    let raw = format!("{}:{}", login.user, login.password);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// A client nonce: unique per request, needn't be secret.
fn new_cnonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    md5_hex(&format!("{now}-{}", COUNTER.fetch_add(1, Ordering::Relaxed)))[..16].to_string()
}

/// A `WWW-Authenticate: Digest ...` challenge (RFC 2617, MD5).
#[derive(Debug, PartialEq)]
struct DigestChallenge {
    realm: String,
    nonce: String,
    opaque: Option<String>,
    /// "auth" when the server offers it.
    qop: Option<String>,
}

impl DigestChallenge {
    fn parse(header: &str) -> Option<DigestChallenge> {
        let rest = header.trim().strip_prefix("Digest").or_else(|| header.trim().strip_prefix("digest"))?;
        let mut params = std::collections::HashMap::new();
        for part in split_params(rest) {
            if let Some((key, value)) = part.split_once('=') {
                params.insert(key.trim().to_ascii_lowercase(), value.trim().trim_matches('"').to_string());
            }
        }
        if params.get("algorithm").is_some_and(|a| !a.eq_ignore_ascii_case("MD5")) {
            return None;
        }
        Some(DigestChallenge {
            realm: params.remove("realm")?,
            nonce: params.remove("nonce")?,
            opaque: params.remove("opaque"),
            qop: params.remove("qop").filter(|q| q.split(',').any(|o| o.trim() == "auth")).map(|_| "auth".into()),
        })
    }

    fn header(&self, login: &Login, method: &str, uri: &str, cnonce: &str) -> String {
        let ha1 = md5_hex(&format!("{}:{}:{}", login.user, self.realm, login.password));
        let ha2 = md5_hex(&format!("{method}:{uri}"));
        let nc = "00000001";
        let response = match &self.qop {
            Some(qop) => md5_hex(&format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", self.nonce)),
            None => md5_hex(&format!("{ha1}:{}:{ha2}", self.nonce)),
        };
        let mut header = format!(
            r#"Digest username="{}", realm="{}", nonce="{}", uri="{uri}", response="{response}", algorithm=MD5"#,
            login.user, self.realm, self.nonce
        );
        if let Some(qop) = &self.qop {
            header.push_str(&format!(r#", qop={qop}, nc={nc}, cnonce="{cnonce}""#));
        }
        if let Some(opaque) = &self.opaque {
            header.push_str(&format!(r#", opaque="{opaque}""#));
        }
        header
    }
}

/// Splits `a="x, y", b=z` at the commas outside quotes.
fn split_params(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted) = (0, false);
    for (i, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_matches_rfc_2617() {
        let challenge = DigestChallenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        let login = Login { user: "Mufasa".into(), password: "Circle Of Life".into() };
        let header = challenge.header(&login, "GET", "/dir/index.html", "0a4f113b");
        assert!(header.contains(r#"response="6629fae49393a05397450978507c4ef1""#), "{header}");
        assert!(header.contains("qop=auth, nc=00000001"), "{header}");
        assert!(header.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#), "{header}");
    }

    #[test]
    fn other_challenges_are_not_digest() {
        assert_eq!(DigestChallenge::parse(r#"Basic realm="CWA""#), None);
        assert_eq!(DigestChallenge::parse(r#"Digest realm="x", nonce="y", algorithm=SHA-256"#), None);
    }

    #[test]
    fn request_uris_keep_the_query() {
        assert_eq!(request_uri("https://h/opds/search?query=a%20b"), "/opds/search?query=a%20b");
        assert_eq!(request_uri("https://h/opds"), "/opds");
    }

    #[test]
    fn basic_auth() {
        let login = Login { user: "Aladdin".into(), password: "open sesame".into() };
        assert_eq!(basic_header(&login), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
    }
}
