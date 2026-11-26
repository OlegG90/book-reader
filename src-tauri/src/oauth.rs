//! OAuth login plumbing shared by the cloud stores: PKCE (RFC 7636) over a
//! loopback address. Each store builds its own authorize address; the
//! browser's answer is read the same way everywhere (RFC 8252 §7.3: an
//! external browser, a loopback redirect back into the app).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// How long `connect` waits for the browser to come back with the code.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// An OAuth login waiting for the browser: open `url`, then `finish` it.
pub struct PendingLogin {
    pub verifier: String,
    pub state: String,
    pub listener: Loopback,
    pub url: String,
}

/// The loopback sockets a login listens on: IPv6 and IPv4, since `localhost`
/// may resolve to either, and never any other interface (RFC 8252 §8.3), so
/// the login port can't be reached from the network.
pub struct Loopback(Vec<TcpListener>);

/// A random alphanumeric string, for PKCE verifiers and login states.
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

/// Listens on both loopback addresses where the system has them, so the
/// browser always lands. The port is fixed per store: it is part of the
/// registered redirect URI.
pub fn bind(port: u16) -> Result<Loopback, String> {
    let mut listeners = Vec::new();
    let mut failure = None;
    for address in [format!("[::1]:{port}"), format!("127.0.0.1:{port}")] {
        match TcpListener::bind(&address).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
            Ok(listener) => listeners.push(listener),
            // Another program on the port would catch the browser's answer.
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                return Err(format!("login: the redirect port is busy ({e})"));
            }
            // A system without IPv6 (or IPv4) loopback just has one address.
            Err(e) => failure = Some(e),
        }
    }
    match failure {
        Some(e) if listeners.is_empty() => Err(format!("login: no loopback address to listen on ({e})")),
        _ => Ok(Loopback(listeners)),
    }
}

/// Waits up to `LOGIN_TIMEOUT` for the browser to come back with `?code=`
/// (or an error, when the reader declines in the browser). Requests that
/// are not this login's answer, such as a browser's favicon or preconnect,
/// are turned away and the wait goes on.
pub fn wait_for_code(listener: Loopback, state: &str, service: &str) -> Result<String, String> {
    let start = Instant::now();
    loop {
        for socket in &listener.0 {
            // An accept error means nothing is waiting, or a connection
            // dropped before it was taken: either way the wait goes on.
            if let Ok((stream, _)) = socket.accept() {
                if let Some(answer) = read_answer(stream, state, service) {
                    return answer;
                }
            }
        }
        if start.elapsed() > LOGIN_TIMEOUT {
            return Err(format!("Timed out waiting for {service} in the browser."));
        }
        std::thread::sleep(Duration::from_millis(50));
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

/// Reads one request and, when it carries this login's `state`, the code or
/// the refusal in it; the tab is told how it went. Anything else (another
/// path, a forged or missing state, a request that never arrives) gets a
/// 404 and `None`, so the login keeps waiting.
fn read_answer(stream: TcpStream, state: &str, service: &str) -> Option<Result<String, String>> {
    // The accepted socket may inherit the listener's non-blocking mode (it
    // does on Windows), and the request bytes may not have arrived yet.
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut first = String::new();
    BufReader::new(&stream).read_line(&mut first).ok()?;
    let answer = first.split_whitespace().nth(1).map(|target| format!("http://localhost{target}"));
    let Some(answer) = answer.filter(|a| state_of(a).as_deref() == Some(state)) else {
        respond(&stream, "404 Not Found", "Not found.");
        return None;
    };
    let result = code_in_answer(&answer, state, service);
    let text = if result.is_ok() { "Connected — you can close this tab." } else { "The login did not go through — you can close this tab." };
    respond(&stream, "200 OK", text);
    Some(result)
}

/// The `state` an answer address carries, if any.
fn state_of(answer: &str) -> Option<String> {
    url::Url::parse(answer).ok()?.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())
}

/// Writes a tiny HTML page as the whole HTTP reply.
fn respond(mut stream: &TcpStream, status: &str, text: &str) {
    let body = format!("<html><body><p>{text}</p></body></html>");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

    #[test]
    fn listens_on_loopback_only() {
        let listener = bind(0).unwrap();
        assert!(!listener.0.is_empty());
        assert!(listener.0.iter().all(|l| l.local_addr().unwrap().ip().is_loopback()));
    }

    #[test]
    fn stray_requests_do_not_end_the_login() {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let port = socket.local_addr().unwrap().port();
        let browser = std::thread::spawn(move || {
            let get = |target: &str| {
                let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(stream, "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
                let mut reply = String::new();
                let _ = io::Read::read_to_string(&mut stream, &mut reply);
                reply
            };
            // A connection that sends nothing, a favicon, a forged state, then the answer.
            drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
            assert!(get("/favicon.ico").starts_with("HTTP/1.1 404"));
            assert!(get("/dropbox?code=evil&state=forged").starts_with("HTTP/1.1 404"));
            assert!(get("/dropbox?code=abc&state=s1").starts_with("HTTP/1.1 200"));
        });
        assert_eq!(wait_for_code(Loopback(vec![socket]), "s1", "Dropbox").unwrap(), "abc");
        browser.join().unwrap();
    }
}
