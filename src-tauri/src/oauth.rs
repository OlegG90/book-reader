//! OAuth login plumbing shared by the cloud stores: PKCE (RFC 7636) over a
//! loopback address. Each store builds its own authorize address; the
//! browser's answer is read the same way everywhere (RFC 8252 §7.3: an
//! external browser, a loopback redirect back into the app).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

/// How long `connect` waits for the browser to come back with the code.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// An OAuth login waiting for the browser: open `url`, then `finish` it.
pub struct PendingLogin {
    pub verifier: String,
    pub state: String,
    pub listener: TcpListener,
    pub url: String,
}

pub fn random_string(len: usize) -> String {
    use rand::{distr::Alphanumeric, Rng};
    rand::rng().sample_iter(&Alphanumeric).take(len).map(char::from).collect()
}

/// The S256 challenge for a verifier (RFC 7636 test vector covered in the stores' tests).
pub fn challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Binds the loopback address (dual-stack where possible, else IPv4), so
/// the browser always lands. The port is fixed per store: it is part of the
/// registered redirect URI.
pub fn bind(port: u16) -> Result<TcpListener, String> {
    TcpListener::bind(format!("[::]:{port}"))
        .or_else(|_| TcpListener::bind(format!("127.0.0.1:{port}")))
        .map_err(|e| format!("login: the redirect port is busy ({e})"))
}

/// Waits up to `LOGIN_TIMEOUT` for the browser to come back with `?code=`
/// (or an error, when the reader declines in the browser).
pub fn wait_for_code(listener: TcpListener, state: &str, service: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| format!("login: {e}"))?;
    let start = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => return read_code(&stream, state, service),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                if start.elapsed() > LOGIN_TIMEOUT {
                    return Err(format!("Timed out waiting for {service} in the browser."));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("login: {e}")),
        }
    }
}

/// The code in the address the browser came back to, if it belongs to this
/// login (`state`). `answer` is the full redirect address: a loopback URL on
/// the desktop, the app's own scheme on a phone.
pub fn code_in_answer(answer: &str, state: &str, service: &str) -> Result<String, String> {
    let query: HashMap<_, _> = url::Url::parse(answer).map_err(|_| "login: the browser sent nothing usable")?.query_pairs().into_owned().collect();
    if let Some(error) = query.get("error") {
        return Err(format!("login: {service} said {error}"));
    }
    if query.get("state").map(String::as_str) != Some(state) {
        return Err("login: the answer is not for this login".into());
    }
    query.get("code").cloned().ok_or("login: the browser sent no code".into())
}

/// Reads the browser's answer (`GET /?code=…&state=…`), checks it answers
/// this login, and tells the tab it can close.
fn read_code(stream: &std::net::TcpStream, state: &str, service: &str) -> Result<String, String> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| format!("login: {e}"))?;
    let mut first = String::new();
    BufReader::new(stream).read_line(&mut first).map_err(|e| format!("login: {e}"))?;
    let target = first.split_whitespace().nth(1).ok_or("login: the browser sent nothing usable")?;
    let code = code_in_answer(&format!("http://localhost{target}"), state, service)?;
    respond(stream, "Connected — you can close this tab.");
    Ok(code)
}

fn respond(stream: &std::net::TcpStream, text: &str) {
    let body = format!("<html><body><p>{text}</p></body></html>");
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_give_the_code_only_with_the_right_state() {
        let loopback = "http://localhost:51944/googledrive?code=abc&state=s1";
        assert_eq!(code_in_answer(loopback, "s1", "Google").unwrap(), "abc");
        // A phone's custom scheme carries the same query.
        let scheme = "com.olegg90.bookreader:/oauth2redirect?code=abc&state=s1";
        assert_eq!(code_in_answer(scheme, "s1", "Google").unwrap(), "abc");
        assert!(code_in_answer(scheme.replace("state=s1", "state=forged").as_str(), "s1", "Google").is_err());
        assert!(code_in_answer("com.olegg90.bookreader:/oauth2redirect?error=access_denied&state=s1", "s1", "Google").is_err());
        assert!(code_in_answer("com.olegg90.bookreader:/oauth2redirect?state=s1", "s1", "Google").is_err());
    }
}
