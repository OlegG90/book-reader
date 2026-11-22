//! HTTP for the catalogs and sync: GET and PUT with the server's login,
//! HTTP Basic (Calibre-Web-Automated) or Digest (the Calibre Content Server).

use md5::{Digest, Md5};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use ureq::config::RedirectAuthHeaders;
use ureq::{Agent, ResponseExt};

/// A server login.
#[derive(Clone)]
pub struct Login {
    pub user: String,
    pub password: String,
}

/// How a request offers the login.
#[derive(Clone, Copy, PartialEq)]
pub enum Auth {
    /// When the server asks for it, the way it asks: a Digest server never
    /// sees the password itself. A server that asked for Basic once gets it
    /// with the first request from then on.
    OnChallenge,
    /// Basic with the first request, for a service that never asks (CWA's KOSync).
    Basic,
}

/// Servers (origins) that asked for Basic: they get it without asking again.
static BASIC_SERVERS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn asks_for_basic(origin: &str) -> bool {
    BASIC_SERVERS.lock().unwrap().as_ref().is_some_and(|servers| servers.contains(origin))
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
        .redirect_auth_headers(RedirectAuthHeaders::SameHost)
        .build()
        .into()
}

/// GET `url`, with the login if there is one.
pub fn get(url: &str, login: Option<&Login>, auth: Auth, accept: &str) -> Result<Reply, String> {
    send("GET", url, login, auth, accept, None)
}

/// PUT a JSON body to `url`.
pub fn put_json(url: &str, login: Option<&Login>, auth: Auth, accept: &str, body: &str) -> Result<Reply, String> {
    send("PUT", url, login, auth, accept, Some(body))
}

fn send(method: &str, url: &str, login: Option<&Login>, auth: Auth, accept: &str, body: Option<&str>) -> Result<Reply, String> {
    let agent = agent();
    let origin = origin_of(url);
    let basic_first = login.is_some() && (auth == Auth::Basic || asks_for_basic(&origin));
    let first = if basic_first { login.map(basic_header) } else { None };
    let mut reply = request(&agent, method, url, accept, first.as_deref(), body)?;
    let Some(login) = login.filter(|_| reply.status == 401 && !basic_first) else { return Ok(reply.reply) };
    let challenge = reply.challenge.clone().unwrap_or_default();
    let header = match DigestChallenge::parse(&challenge) {
        Some(digest) => digest.header(login, method, &request_uri(url), &new_cnonce()),
        None if challenge.trim_start().to_ascii_lowercase().starts_with("basic") => basic_header(login),
        None => return Ok(reply.reply),
    };
    reply = request(&agent, method, url, accept, Some(&header), body)?;
    if reply.status != 401 && header.starts_with("Basic") {
        BASIC_SERVERS.lock().unwrap().get_or_insert_with(HashSet::new).insert(origin);
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
    // A redirect may not lead off the server the request was meant for.
    if origin_of(&response.get_uri().to_string()) != origin_of(url) {
        return Err(format!("{} sent the request on to another server", host_of(url)));
    }
    let status = response.status().as_u16();
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let challenge = header("www-authenticate");
    let mut response = response;
    let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(failed)?;
    Ok(Answer { reply: Reply { status, body }, challenge })
}

/// Scheme, host and port of a URL ("" if it isn't one).
fn origin_of(url: &str) -> String {
    url::Url::parse(url).map(|u| u.origin().ascii_serialization()).unwrap_or_default()
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
