//! Catalog sources (OPDS servers) and what the catalog screen asks of them:
//! a feed, a search, a cover, a book file. Every request goes to a source's
//! own server, so a feed or a book can't make the app fetch anything else.

use crate::net::{self, Auth, Login, Reply};
use crate::opds::{self, Feed, FileFormat};
use crate::platform::{Active, Platform};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use url::Url;

/// An OPDS server: Calibre-Web-Automated, the Calibre Content Server...
/// Its password is kept in the OS credential store, never here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub name: String,
    /// The OPDS root, e.g. `https://books.example/opds`.
    pub url: String,
    #[serde(default)]
    pub user: String,
    /// The source that syncs reading progress (KOSync on CWA).
    #[serde(default)]
    pub sync: bool,
}

const FEED_TYPES: &str = "application/atom+xml, application/xml;q=0.9, */*;q=0.5";

/// A catalog address the app accepts: http(s) only.
pub fn check_url(url: &str) -> Result<Url, String> {
    let parsed = Url::parse(url.trim()).map_err(|_| format!("Not a web address: {url}"))?;
    match parsed.scheme() {
        "http" | "https" if parsed.host_str().is_some() => Ok(parsed),
        _ => Err(format!("Not a web address: {url}")),
    }
}

/// A warning for plain HTTP across the internet, where the password travels
/// unencrypted; home networks and Tailscale are fine.
pub fn insecure_warning(url: &Url) -> Option<String> {
    if url.scheme() == "https" {
        return None;
    }
    let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
    let local = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_private() || ip.is_loopback() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        // Loopback, unique local (fc00::/7) and link-local (fe80::/10).
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => host == "localhost" || host.ends_with(".local") || host.ends_with(".lan") || host.ends_with(".ts.net") || !host.contains('.'),
    };
    (!local).then(|| {
        format!("{host} is reached over plain HTTP: the password and the books travel unencrypted. Use https:// if the server has it.")
    })
}

/// Whether `url` is on the source's own server (scheme, host and port).
fn on_server(source: &Source, url: &str) -> bool {
    match (Url::parse(&source.url), Url::parse(url)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}

/// The source's login, with the password from the credential store.
pub fn login(source: &Source) -> Option<Login> {
    login_with(source, None)
}

/// The source's login with `password`, or the kept one when that is None.
pub fn login_with(source: &Source, password: Option<&str>) -> Option<Login> {
    (!source.user.is_empty()).then(|| Login {
        user: source.user.clone(),
        password: password.map(str::to_string).or_else(|| kept_password(source)).unwrap_or_default(),
    })
}

/// The source's kept password. A source whose id is a cloud token's never
/// gets one, so a refresh token can't leave as a catalog's Basic auth.
fn kept_password(source: &Source) -> Option<String> {
    (!crate::cloud::is_token_id(&source.id)).then(|| Active::password(&source.id)).flatten()
}

/// What a refused login reads as.
pub fn refused(server: &str) -> String {
    format!("{server} didn't accept the user name or password")
}

/// GET from the source's server, with its login; failures read as sentences.
fn fetch(source: &Source, url: &str, accept: &str) -> Result<Reply, String> {
    if !on_server(source, url) {
        return Err(format!("{url} is not on {}", source.name));
    }
    let reply = net::get(url, login(source).as_ref(), Auth::OnChallenge, accept)?;
    match reply.status {
        200..=299 => Ok(reply),
        401 | 403 => Err(refused(&source.name)),
        404 => Err(format!("{} has nothing at {url}", source.name)),
        status => Err(format!("{} answered with an error ({status})", source.name)),
    }
}

/// Finds the OPDS catalog behind the address the reader typed: the address
/// itself if it is a catalog, else `/opds` under it or at the server's root
/// (where Calibre-Web, CWA and the Calibre Content Server all keep theirs).
/// Returns the catalog's address. `login` is the one being saved, so a
/// refused password never replaces a working one.
pub fn discover(source: &Source, login: Option<&Login>) -> Result<Url, String> {
    let typed = check_url(&source.url)?;
    let mut candidates = vec![typed.clone()];
    let under = Url::parse(&format!("{}/opds", typed.as_str().trim_end_matches('/')));
    for candidate in [under, typed.join("/opds")].into_iter().flatten() {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    for candidate in &candidates {
        let reply = net::get(candidate.as_str(), login, Auth::OnChallenge, FEED_TYPES)?;
        match reply.status {
            401 | 403 => return Err(refused(typed.host_str().unwrap_or("The server"))),
            200..=299 if opds::parse_feed(&String::from_utf8_lossy(&reply.body), candidate.as_str()).is_ok() => return Ok(candidate.clone()),
            _ => {}
        }
    }
    let tried: Vec<&str> = candidates.iter().map(Url::as_str).collect();
    Err(format!("No OPDS catalog found: tried {}", tried.join(", ")))
}

/// A page of the catalog: the source's root, or `url` on its server.
pub fn feed(source: &Source, url: Option<&str>) -> Result<Feed, String> {
    let url = url.unwrap_or(&source.url);
    let reply = fetch(source, url, FEED_TYPES)?;
    opds::parse_feed(&String::from_utf8_lossy(&reply.body), url)
        .map_err(|_| format!("{} didn't send a catalog page: is {} its OPDS address?", source.name, source.url))
}

/// The source's search URL template (from its OpenSearch description).
pub fn search_template(source: &Source) -> Result<String, String> {
    let description = feed(source, None)?.search.ok_or_else(|| format!("{} has no search", source.name))?;
    let reply = fetch(source, &description, "application/opensearchdescription+xml, */*;q=0.5")?;
    opds::parse_search_template(&String::from_utf8_lossy(&reply.body), &description).map_err(|e| e.to_string())
}

/// The books matching `query`, with a search template from `search_template`.
pub fn search(source: &Source, template: &str, query: &str) -> Result<Feed, String> {
    feed(source, Some(&opds::search_url(template, query)))
}

/// A cover image.
pub fn image(source: &Source, url: &str) -> Result<Vec<u8>, String> {
    Ok(fetch(source, url, "image/*")?.body)
}

/// A book file, saved under `dir` with its format's extension; returns its path.
pub fn download(source: &Source, url: &str, format: FileFormat, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let reply = fetch(source, url, "*/*")?;
    let path = dir.join(format!("download-{}.{}", crate::clock::now_nanos(), format.extension()));
    crate::store::write_atomically(&path, &reply.body).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str) -> Source {
        Source { id: "1".into(), name: "CWA".into(), url: url.into(), user: String::new(), sync: false }
    }

    #[test]
    fn a_source_with_a_cloud_tokens_id_gets_no_password() {
        let mut hijack = source("https://books.example/opds");
        hijack.id = crate::google::TOKEN_ID.into();
        hijack.user = "me".into();
        // Never reaches the credential store, so the token can't leave as Basic auth.
        assert_eq!(login(&hijack).unwrap().password, "");
    }

    #[test]
    fn only_the_sources_own_server() {
        let cwa = source("https://books.example/opds");
        assert!(on_server(&cwa, "https://books.example/opds/download/1/epub/"));
        assert!(!on_server(&cwa, "http://books.example/opds"), "another scheme");
        assert!(!on_server(&cwa, "https://books.example:8443/opds"), "another port");
        assert!(!on_server(&cwa, "https://evil.example/opds"));
        assert!(fetch(&cwa, "https://evil.example/x", "*/*").unwrap_err().contains("not on CWA"));
    }

    #[test]
    fn plain_http_over_the_internet_is_flagged() {
        let warn = |url: &str| insecure_warning(&check_url(url).unwrap());
        assert!(warn("https://books.example/opds").is_none());
        assert!(warn("http://192.168.1.10:8083/opds").is_none());
        assert!(warn("http://100.101.102.103/opds").is_none(), "Tailscale");
        assert!(warn("http://nas.tail1234.ts.net/opds").is_none());
        assert!(warn("http://nas/opds").is_none(), "a name on the home network");
        assert!(warn("http://books.example/opds").is_some());
        assert!(warn("http://8.8.8.8/opds").is_some());
        assert!(warn("http://[fd00::5]:8083/opds").is_none(), "unique local IPv6");
        assert!(warn("http://[2001:4860::8888]/opds").is_some(), "public IPv6");
    }

    #[test]
    fn addresses_must_be_web_addresses() {
        assert!(check_url("ftp://x/opds").is_err());
        assert!(check_url("books.example/opds").is_err());
        assert!(check_url(" https://books.example/opds ").is_ok());
    }
}
