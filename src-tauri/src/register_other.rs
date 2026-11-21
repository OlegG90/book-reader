//! File associations are a Windows feature (see register.rs); elsewhere
//! (Android) the same calls report that they are not available.

use std::io;
use std::path::Path;

pub const EXTENSIONS: &[&str] = &[];

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "File associations are only available on Windows")
}

pub fn register(_exe: &Path) -> io::Result<()> {
    Err(unsupported())
}

pub fn unregister() -> io::Result<()> {
    Err(unsupported())
}

pub fn registered() -> bool {
    false
}
