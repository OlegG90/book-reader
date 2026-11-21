use std::path::Path;

/// Whether two paths name the same file, as Windows sees it: resolving `.`
/// and `..` and ignoring case.
pub fn same_file(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// A file's identity as Windows sees it: the absolute path (which resolves
/// `.` and `..`), ignoring case.
pub fn path_key(path: &Path) -> String {
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    full.to_string_lossy().to_lowercase()
}

/// The file name, for showing to the reader.
pub fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_file_ignores_case_and_dots() {
        assert!(same_file(Path::new(r"C:\Books\A.EPUB"), Path::new(r"c:\books\x\..\.\a.epub")));
        assert!(!same_file(Path::new(r"C:\Books\a.epub"), Path::new(r"C:\Books\b.epub")));
    }
}
