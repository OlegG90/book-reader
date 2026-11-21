//! KOReader progress sync (KOSync), as Calibre-Web-Automated serves it.

use md5::{Digest, Md5};
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
    fn small_files_hash_what_there_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.fb2");
        fs::write(&path, b"tiny").unwrap();
        assert_eq!(document_id(&path).unwrap(), expected(b"tiny"));
    }
}
