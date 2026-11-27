//! HTTP for the catalogs and sync: GET and PUT with the server's login,
//! HTTP Basic (Calibre-Web-Automated) or Digest (the Calibre Content Server).

use md5::{Digest, Md5};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
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

#[cfg(test)]
impl Reply {
    /// An answer for tests elsewhere.
    pub fn for_test(status: u16, body: &[u8]) -> Self {
        Reply { status, body: body.to_vec(), challenge: None }
    }
}

/// Servers (origins) that asked for Basic: they get it without asking again.
static BASIC_SERVERS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// What a request brought back.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// The `WWW-Authenticate` header, on a 401.
    challenge: Option<String>,
}

/// The largest download accepted: a big illustrated book, with room to spare.
const MAX_BODY: u64 = 300 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

/// One client for the app, so connections to a server are reused.
fn agent() -> &'static Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        Agent::config_builder()
            // 401 is an answer to act on (Digest), not an error.
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .user_agent(concat!("BookReader/", env!("CARGO_PKG_VERSION")))
            .redirect_auth_headers(RedirectAuthHeaders::SameHost)
            .build()
            .into()
    })
}

/// GET `url`, with the login if there is one.
pub fn get(url: &str, login: Option<&Login>, auth: Auth, accept: &str) -> Result<Reply, String> {
    send(url, login, auth, accept, None)
}

/// PUT a JSON body to `url`.
pub fn put_json(url: &str, login: Option<&Login>, auth: Auth, accept: &str, body: &str) -> Result<Reply, String> {
    send(url, login, auth, accept, Some(body))
}

/// POST bytes to `url` with extra headers (a sync store's API).
pub fn post(url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Reply, String> {
    send_bytes(agent().post(url), url, headers, body)
}

/// PATCH bytes to `url` with extra headers (a sync store's API).
pub fn patch(url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Reply, String> {
    send_bytes(agent().patch(url), url, headers, body)
}

/// PUT bytes to `url` with extra headers (a sync store's API).
pub fn put(url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Reply, String> {
    send_bytes(agent().put(url), url, headers, body)
}

/// Sends a request with a body and extra headers, and reads the answer.
fn send_bytes(mut req: ureq::RequestBuilder<ureq::typestate::WithBody>, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<Reply, String> {
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    finish(req.send(body), url)
}

/// DELETE with a bearer token.
pub fn bearer_delete(url: &str, token: &str) -> Result<Reply, String> {
    let auth = format!("Bearer {token}");
    finish(agent().delete(url).header("Authorization", auth.as_str()).call(), url)
}

/// GET `url` with a Bearer token (a sync store's API).
pub fn bearer_get(url: &str, token: &str) -> Result<Reply, String> {
    let auth = format!("Bearer {token}");
    finish(agent().get(url).header("Authorization", auth.as_str()).call(), url)
}

/// A GET, or with a body a PUT.
fn send(url: &str, login: Option<&Login>, auth: Auth, accept: &str, body: Option<&str>) -> Result<Reply, String> {
    let origin = origin_of(url);
    let basic_first = login.is_some() && (auth == Auth::Basic || BASIC_SERVERS.lock().unwrap().contains(&origin));
    let first = if basic_first { login.map(basic_header) } else { None };
    let reply = request(url, accept, first.as_deref(), body)?;
    let Some(login) = login.filter(|_| reply.status == 401 && !basic_first) else { return Ok(reply) };
    let challenge = reply.challenge.as_deref().unwrap_or_default();
    let header = match DigestChallenge::parse(challenge) {
        Some(digest) => {
            let method = if body.is_some() { "PUT" } else { "GET" };
            digest.header(login, method, &request_uri(url), &new_cnonce())
        }
        None if challenge.trim_start().to_ascii_lowercase().starts_with("basic") => basic_header(login),
        None => return Ok(reply),
    };
    let reply = request(url, accept, Some(&header), body)?;
    if reply.status != 401 && header.starts_with("Basic") {
        BASIC_SERVERS.lock().unwrap().insert(origin);
    }
    Ok(reply)
}

/// The headers every request carries.
fn headers<B>(req: ureq::RequestBuilder<B>, accept: &str, auth: Option<&str>) -> ureq::RequestBuilder<B> {
    let req = req.header("Accept", accept);
    match auth {
        Some(auth) => req.header("Authorization", auth),
        None => req,
    }
}

fn request(url: &str, accept: &str, auth: Option<&str>, body: Option<&str>) -> Result<Reply, String> {
    let response = match body {
        Some(body) => headers(agent().put(url), accept, auth).header("Content-Type", "application/json").send(body),
        None => headers(agent().get(url), accept, auth).call(),
    };
    finish(response, url)
}

/// Reads a request's answer, failing closed on errors and off-server redirects.
fn finish(response: Result<ureq::http::Response<ureq::Body>, ureq::Error>, url: &str) -> Result<Reply, String> {
    let failed = |e: ureq::Error| format!("{}: {e}", host_of(url));
    let mut response = response.map_err(failed)?;
    // A redirect may not lead off the server the request was meant for.
    if origin_of(&response.get_uri().to_string()) != origin_of(url) {
        return Err(format!("{} sent the request on to another server", host_of(url)));
    }
    let status = response.status().as_u16();
    let challenge = response.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(failed)?;
    Ok(Reply { status, body, challenge })
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
    use url::Position;
    url::Url::parse(url).map_or_else(|_| url.to_string(), |u| u[Position::BeforePath..Position::AfterQuery].to_string())
}

fn basic_header(login: &Login) -> String {
    use base64::Engine;
    let raw = format!("{}:{}", login.user, login.password);
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

fn md5_hex(text: &str) -> String {
    format!("{:x}", Md5::digest(text.as_bytes()))
}

/// A client nonce: unique per request, needn't be secret.
fn new_cnonce() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    md5_hex(&format!("{}-{}", crate::clock::now_nanos(), COUNTER.fetch_add(1, Ordering::Relaxed)))[..16].to_string()
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
