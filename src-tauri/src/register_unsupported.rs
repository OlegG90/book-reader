//! File associations are a Windows feature (see register.rs); elsewhere
//! (Android) the same calls report that they are not available.

use std::io;
use std::path::Path;

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "File associations are only available on Windows")
}

/// Always fails: see the module docs.
pub fn register(_exe: &Path) -> io::Result<()> {
    Err(unsupported())
}

/// Always fails: see the module docs.
pub fn unregister() -> io::Result<()> {
    Err(unsupported())
}

/// Never registered.
pub fn registered() -> bool {
    false
}
