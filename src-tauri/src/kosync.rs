//! KOReader progress sync (KOSync), as Calibre-Web-Automated serves it: at
//! `/kosync` on the server, with the CWA login over HTTP Basic.

use crate::catalog::{self, Source};
use crate::net::{self, Auth};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// Bytes read at each sample point.
const SAMPLE: u64 = 1024;
/// Sample points: 0, then 1 KiB × 4ⁿ (1K, 4K, 16K … 1G), as KOReader's
/// `partialMD5` reads them (its first shift, by -2, comes out as 0).
const SAMPLES: u32 = 12;

/// The document id KOSync knows a book by: KOReader's partial MD5, an MD5 over
/// 1 KiB samples spread through the file. CWA computes the same for the files
/// in its library, so a book downloaded from it matches its library entry.
pub fn document_id(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut md5 = Md5::new();
    let mut buffer = [0u8; SAMPLE as usize];
    for i in 0..SAMPLES {
        let offset = if i == 0 { 0 } else { SAMPLE << (2 * (i - 1)) };
        file.seek(SeekFrom::Start(offset))?;
        let read = read_up_to(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        md5.update(&buffer[..read]);
    }
    Ok(md5.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

const ACCEPT: &str = "application/vnd.koreader.v1+json";

/// A reading position as the server keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Where in the book: a CFI from Book Reader (KOReader puts its own xpointers here).
    pub progress: String,
    /// Share read, 0–1.
    pub percentage: f64,
    /// Which device: "Book Reader on Android"…
    pub device: String,
    pub device_id: String,
    /// When the server got it, in seconds since the Unix epoch.
    pub timestamp: u64,
}

/// What the server sends back for a book (`{}` when it has nothing).
#[derive(Deserialize)]
struct Stored {
    progress: Option<String>,
    percentage: Option<f64>,
    device: Option<String>,
    device_id: Option<String>,
    timestamp: Option<u64>,
}

/// `https://host/kosync`: the sync service next to the source's catalog.
fn base(source: &Source) -> Result<String, String> {
    let url = catalog::check_url(&source.url)?;
    Ok(format!("{}/kosync", url.origin().ascii_serialization()))
}

/// Turns the server's answer into a result, with CWA's errors as sentences.
fn check(source: &Source, reply: net::Reply) -> Result<net::Reply, String> {
    match reply.status {
        200..=299 => Ok(reply),
        503 => Err(format!("Progress sync is turned off on {}", source.name)),
        // CWA answers a refused login with 400 and error 2001.
        400 if String::from_utf8_lossy(&reply.body).contains("2001") => Err(catalog::refused(&source.name)),
        401 | 403 => Err(catalog::refused(&source.name)),
        status => Err(format!("{} answered with an error ({status})", source.name)),
    }
}

/// The position the server has for a book, if any.
pub fn pull(source: &Source, document: &str) -> Result<Option<Progress>, String> {
    let url = format!("{}/syncs/progress/{document}", base(source)?);
    let reply = check(source, net::get(&url, catalog::login(source).as_ref(), Auth::Basic, ACCEPT)?)?;
    let stored: Stored = serde_json::from_slice(&reply.body).map_err(|e| e.to_string())?;
    let (Some(progress), Some(percentage)) = (stored.progress, stored.percentage) else { return Ok(None) };
    Ok(Some(Progress {
        progress,
        // CWA stores percents; the protocol speaks of a 0–1 share.
        percentage: if percentage > 1.0 { percentage / 100.0 } else { percentage },
        device: stored.device.unwrap_or_default(),
        device_id: stored.device_id.unwrap_or_default(),
        timestamp: stored.timestamp.unwrap_or_default(),
    }))
}

/// Sends a book's position to the server (which stamps the time itself).
pub fn push(source: &Source, document: &str, progress: &str, percentage: f64, device: &str, device_id: &str) -> Result<(), String> {
    #[derive(Serialize)]
    struct Update<'a> {
        document: &'a str,
        progress: &'a str,
        percentage: f64,
        device: &'a str,
        device_id: &'a str,
    }
    let body = serde_json::to_string(&Update { document, progress, percentage, device, device_id }).map_err(|e| e.to_string())?;
    let url = format!("{}/syncs/progress", base(source)?);
    check(source, net::put_json(&url, catalog::login(source).as_ref(), Auth::Basic, ACCEPT, &body)?).map(|_| ())
}

/// Reads until `buffer` is full or the file ends.
fn read_up_to(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// The algorithm written out plainly, to check the loop against.
    fn expected(bytes: &[u8]) -> String {
        let mut md5 = Md5::new();
        for offset in [0usize, 1024, 4096, 16384, 65536, 262144] {
            if offset >= bytes.len() {
                break;
            }
            md5.update(&bytes[offset..(offset + 1024).min(bytes.len())]);
        }
        md5.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn samples_at_koreaders_offsets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        assert_eq!(document_id(&path).unwrap(), expected(&bytes));
    }

    #[test]
    fn the_sync_service_sits_at_the_servers_root() {
        let source = Source { id: "1".into(), name: "CWA".into(), url: "http://192.168.1.5:8083/opds".into(), user: "u".into(), sync: true };
        assert_eq!(base(&source).unwrap(), "http://192.168.1.5:8083/kosync");
    }

    #[test]
    fn small_files_hash_what_there_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.fb2");
        fs::write(&path, b"tiny").unwrap();
        assert_eq!(document_id(&path).unwrap(), expected(b"tiny"));
    }
}
