//! Where a book goes in the library:
//! `{Last First}/{Series} {NN} - {Title}.{ext}`, or `{Last First}/{Title}.{ext}`
//! without a series, and `Unknown Author/...` without an author.

use crate::fingerprint::fingerprint;
use crate::metadata::{Author, Book, Format};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

const UNKNOWN_AUTHOR: &str = "Unknown Author";
/// Longest folder or file name (in characters) the template produces.
const MAX_NAME: usize = 120;

/// The book's path inside the library, with `/` separators.
pub fn template_path(book: &Book, format: Format) -> String {
    let author = book.authors.first().map(Author::sort_name).filter(|a| !a.is_empty());
    let folder = sanitize(author.as_deref().unwrap_or(UNKNOWN_AUTHOR));
    let title = if book.title.trim().is_empty() { "Untitled" } else { &book.title };
    let stem = match book.series.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(series) => match book.series_index.as_deref().map(pad_index) {
            Some(index) => format!("{series} {index} - {title}"),
            None => format!("{series} - {title}"),
        },
        None => title.to_string(),
    };
    format!("{folder}/{}.{}", sanitize(&stem), format.extension())
}

/// "3" -> "03", so books in a series list in order; "2.5" stays as is.
fn pad_index(index: &str) -> String {
    match index.trim().parse::<u32>() {
        Ok(n) => format!("{n:02}"),
        Err(_) => index.trim().to_string(),
    }
}

/// Makes one path component safe for Windows (and Android): a subtitle colon
/// becomes " - ", other invalid characters `_`, trailing dots and spaces go,
/// reserved device names get a `_`.
fn sanitize(name: &str) -> String {
    let replaced: String = name
        .replace(": ", " - ")
        .chars()
        .map(|c| if c.is_control() || r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut short: String = collapsed.chars().take(MAX_NAME).collect();
    while short.ends_with(['.', ' ']) {
        short.pop();
    }
    if short.is_empty() {
        return "_".into();
    }
    let stem = short.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        short.insert(0, '_');
    }
    short
}

/// Where a file should land.
#[derive(Debug, PartialEq)]
pub enum Placement {
    /// A free path (relative, `/` separators).
    Free(String),
    /// The library already holds this very book here.
    Exists(String),
}

/// Finds where `source` can go, starting from its template path: a file with
/// the same content there means the book is already in the library; a
/// different one makes it try ` (2)`, ` (3)`, ... `taken` lists paths other
/// planned moves will use, which count as occupied.
pub fn place(root: &Path, wanted: &str, source_fingerprint: &str, taken: &HashSet<String>) -> io::Result<Placement> {
    let (stem, ext) = split_ext(wanted);
    for n in 1.. {
        let candidate = if n == 1 { wanted.to_string() } else { format!("{stem} ({n}){ext}") };
        if taken.contains(&candidate.to_lowercase()) {
            continue;
        }
        let full = root.join(&candidate);
        if !full.exists() {
            return Ok(Placement::Free(candidate));
        }
        if fingerprint(&full)? == source_fingerprint {
            return Ok(Placement::Exists(candidate));
        }
    }
    unreachable!()
}

/// "A/B.fb2.zip" -> ("A/B", ".fb2.zip"): the book extension stays whole.
fn split_ext(path: &str) -> (&str, &str) {
    let stem = Format::split(path).map_or(path, |(stem, _)| stem);
    path.split_at(stem.len())
}

/// Creates the target's folders; refuses a target that exists, so a book is
/// never overwritten.
fn prepare_target(to: &Path) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", to.display())));
    }
    match to.parent() {
        Some(dir) => std::fs::create_dir_all(dir),
        None => Ok(()),
    }
}

/// Moves a file, also across drives (where a rename can't). If the original
/// can't be removed after copying, the copy is removed again, so the book
/// never ends up in both places.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    prepare_target(to)?;
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from).inspect_err(|_| {
        let _ = std::fs::remove_file(to);
    })
}

pub fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    prepare_target(to)?;
    std::fs::copy(from, to).map(|_| ())
}

/// Removes `dir` and its empty parents, up to (not including) `root`.
pub fn remove_empty_dirs(root: &Path, mut dir: PathBuf) {
    while dir.starts_with(root) && dir != root && std::fs::remove_dir(&dir).is_ok() {
        if !dir.pop() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn author(first: &str, last: &str) -> Author {
        Author { first: first.into(), last: last.into() }
    }

    fn path(title: &str, authors: &[Author], series: Option<&str>, index: Option<&str>, format: Format) -> String {
        let book = Book {
            title: title.into(),
            authors: authors.to_vec(),
            series: series.map(Into::into),
            series_index: index.map(Into::into),
        };
        template_path(&book, format)
    }

    #[test]
    fn template_with_and_without_series() {
        let pratchett = [author("Terry", "Pratchett"), author("Neil", "Gaiman")];
        assert_eq!(path("Guards! Guards!", &pratchett, Some("Discworld"), Some("8"), Format::Epub), "Pratchett Terry/Discworld 08 - Guards! Guards!.epub");
        assert_eq!(path("Good Omens", &pratchett, None, None, Format::Fb2), "Pratchett Terry/Good Omens.fb2");
        assert_eq!(path("Т", &[author("Іван", "Франко")], Some("С"), Some("2.5"), Format::Fb2Zip), "Франко Іван/С 2.5 - Т.fb2.zip");
        assert_eq!(path("Saga", &[author("", "Homer")], Some("Epics"), None, Format::Epub), "Homer/Epics - Saga.epub");
    }

    #[test]
    fn missing_author_or_title() {
        assert_eq!(path("Notes", &[], None, None, Format::Epub), "Unknown Author/Notes.epub");
        assert_eq!(path(" ", &[author("A", "B")], None, None, Format::Epub), "B A/Untitled.epub");
    }

    #[test]
    fn sanitizing_names() {
        assert_eq!(sanitize(r#"What? A "Title": Part 1/2"#), "What_ A _Title_ - Part 1_2");
        assert_eq!(sanitize("Зразок: Містечко"), "Зразок - Містечко");
        assert_eq!(sanitize("12:30"), "12_30");
        assert_eq!(sanitize("Ends with dots..."), "Ends with dots");
        assert_eq!(sanitize("con"), "_con");
        assert_eq!(sanitize("COM1.txt"), "_COM1.txt");
        assert_eq!(sanitize("Comet"), "Comet");
        assert_eq!(sanitize("..."), "_");
        assert_eq!(sanitize(&"я".repeat(300)).chars().count(), MAX_NAME);
    }

    #[test]
    fn placing_next_to_other_books() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("A")).unwrap();
        fs::write(root.join("A/B.fb2.zip"), "other book").unwrap();
        let source = root.join("source.fb2.zip");
        fs::write(&source, "this book").unwrap();
        let fp = fingerprint(&source).unwrap();

        let none = HashSet::new();
        assert_eq!(place(root, "A/B.fb2.zip", &fp, &none).unwrap(), Placement::Free("A/B (2).fb2.zip".into()));
        assert_eq!(place(root, "A/C.epub", &fp, &none).unwrap(), Placement::Free("A/C.epub".into()));

        fs::write(root.join("A/B (2).fb2.zip"), "this book").unwrap();
        assert_eq!(place(root, "A/B.fb2.zip", &fp, &none).unwrap(), Placement::Exists("A/B (2).fb2.zip".into()));

        let taken = HashSet::from(["a/c.epub".to_string()]);
        assert_eq!(place(root, "A/C.epub", &fp, &taken).unwrap(), Placement::Free("A/C (2).epub".into()));
    }

    #[test]
    fn moving_creates_folders_and_cleaning_removes_empty_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("old/deep")).unwrap();
        fs::write(root.join("old/deep/a.epub"), "x").unwrap();
        move_file(&root.join("old/deep/a.epub"), &root.join("new/a.epub")).unwrap();
        assert!(root.join("new/a.epub").is_file());
        remove_empty_dirs(root, root.join("old/deep"));
        assert!(!root.join("old").exists());
        assert!(root.exists());
    }

    #[test]
    fn an_existing_target_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.epub"), dir.path().join("b.epub"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        assert!(move_file(&a, &b).is_err());
        assert!(copy_file(&a, &b).is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        assert!(a.is_file());
    }
}
