//! The reading formats as modules: EPUB, and FB2 (one module covers
//! `.fb2`, `.fb2.zip` and `.fbz`). Each reader knows its files and reads
//! them; the registry answers which reader a file belongs to.

use crate::metadata::{self, Metadata};
use crate::xml::invalid;
use crate::{modules, store};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;
use zip::ZipArchive;

/// One reading format: its module, its files, and how to read them.
pub trait FormatReader: Sync {
    /// The module's id (`format:epub`, `format:fb2`).
    fn module(&self) -> &'static str;
    /// The format's name, for showing ("EPUB", "FB2").
    fn label(&self) -> &'static str;
    /// File name endings, with the dot, in match order (case-insensitive).
    fn suffixes(&self) -> &'static [&'static str];
    /// The file's own ending, without the dot (`fb2.zip` for `x.FB2.ZIP`).
    fn extension_for(&self, name: &str) -> Option<&'static str> {
        self.suffixes().iter().find_map(|suffix| {
            let cut = name.len().checked_sub(suffix.len())?;
            let tail = name.get(cut..)?;
            tail.eq_ignore_ascii_case(suffix).then(|| &suffix[1..])
        })
    }
    /// Reads the book's metadata and cover.
    fn read(&self, path: &Path) -> io::Result<Metadata>;
}

/// EPUB books (`.epub`).
pub struct EpubReader;

/// FB2 books (`.fb2`, `.fb2.zip` and `.fbz`).
pub struct Fb2Reader;

impl FormatReader for EpubReader {
    fn module(&self) -> &'static str {
        "format:epub"
    }

    fn label(&self) -> &'static str {
        "EPUB"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &[".epub"]
    }

    fn read(&self, path: &Path) -> io::Result<Metadata> {
        metadata::epub(BufReader::new(File::open(path)?))
    }
}

impl FormatReader for Fb2Reader {
    fn module(&self) -> &'static str {
        "format:fb2"
    }

    fn label(&self) -> &'static str {
        "FB2"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &[".fb2.zip", ".fbz", ".fb2"]
    }

    fn read(&self, path: &Path) -> io::Result<Metadata> {
        let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        if self.extension_for(&name) == Some("fb2") {
            return metadata::fb2(&std::fs::read(path)?);
        }
        let mut zip = ZipArchive::new(BufReader::new(File::open(path)?)).map_err(metadata::zip_error)?;
        let entry = (0..zip.len())
            .filter_map(|i| zip.name_for_index(i)?.ok().map(|n| n.into_owned()))
            .find(|n| n.to_lowercase().ends_with(".fb2"))
            .ok_or_else(|| invalid("no .fb2 file inside"))?;
        metadata::fb2(&metadata::read_entry(&mut zip, &entry)?)
    }
}

static EPUB: EpubReader = EpubReader;
static FB2: Fb2Reader = Fb2Reader;

/// Every reader, in preference order.
pub fn registry() -> [&'static dyn FormatReader; 2] {
    [&EPUB, &FB2]
}

/// A reader by its module id (`format:epub`, `format:fb2`).
pub fn by_module(id: &str) -> Option<&'static dyn FormatReader> {
    registry().into_iter().find(|r| r.module() == id)
}

/// Splits a file name (or path) into the part before its book extension and
/// the reader; `None` for files the reader doesn't open.
pub fn split(name: &str) -> Option<(&str, &'static dyn FormatReader)> {
    registry().into_iter().find_map(|reader| {
        // The ending matched, so cutting it (and its dot) stays on a boundary.
        let ext = reader.extension_for(name)?;
        Some((&name[..name.len() - ext.len() - 1], reader))
    })
}

/// The reader for a file; `None` for files the reader doesn't open.
pub fn of(path: &Path) -> Option<&'static dyn FormatReader> {
    split(&path.file_name()?.to_string_lossy()).map(|(_, reader)| reader)
}

/// The enabled endings without the dot, for a file dialog's filter.
pub fn enabled_extensions(disabled: &BTreeSet<String>) -> Vec<&'static str> {
    registry()
        .iter()
        .filter(|r| !disabled.contains(r.module()))
        .flat_map(|r| r.suffixes().iter().copied())
        .map(|suffix| &suffix[1..])
        .collect()
}

/// Reads a book file's metadata and cover; fails for files the reader doesn't open.
pub fn read(path: &Path) -> io::Result<Metadata> {
    let reader = of(path).ok_or_else(|| invalid("not a book file"))?;
    reader.read(path)
}

/// Refuses with the Modules sentence when the reader's module is turned off.
pub fn check(state: &store::State, reader: &dyn FormatReader) -> Result<(), String> {
    modules::require(state, reader.module())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn samples() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples")
    }

    #[test]
    fn detects_and_splits_per_format() {
        assert_eq!(of(Path::new("A.EPUB")).unwrap().module(), "format:epub");
        assert_eq!(of(Path::new("a.fb2")).unwrap().module(), "format:fb2");
        assert_eq!(of(Path::new("a.FB2.zip")).unwrap().module(), "format:fb2");
        assert_eq!(of(Path::new("a.fbz")).unwrap().module(), "format:fb2");
        assert!(of(Path::new("a.zip")).is_none());
        assert!(of(Path::new("a.md")).is_none());
        // The archive suffix wins over the plain one.
        let (stem, reader) = split("a.fb2.zip").unwrap();
        assert_eq!(stem, "a");
        assert_eq!(reader.module(), "format:fb2");
        assert_eq!(split("a.FBZ").unwrap().0, "a");
    }

    #[test]
    fn extensions_follow_the_registry() {
        let none = BTreeSet::new();
        assert_eq!(enabled_extensions(&none), ["epub", "fb2.zip", "fbz", "fb2"]);
        let off = BTreeSet::from(["format:fb2".to_string()]);
        assert_eq!(enabled_extensions(&off), ["epub"]);
        assert_eq!(by_module("format:fb2").unwrap().extension_for("a.FB2.ZIP"), Some("fb2.zip"));
        assert_eq!(by_module("format:fb2").unwrap().extension_for("a.fbz"), Some("fbz"));
        assert_eq!(by_module("format:fb2").unwrap().extension_for("a.fb2"), Some("fb2"));
    }

    #[test]
    fn reads_the_sample_books() {
        let alice = read(&samples().join("alice-in-wonderland.epub")).unwrap();
        assert!(alice.book.title.contains("Alice"), "{}", alice.book.title);
        assert_eq!(alice.book.authors[0].last, "Carroll");
        let fb2 = read(&samples().join("sample-windows-1251.fb2")).unwrap();
        assert!(fb2.book.title.chars().any(|c| ('а'..='я').contains(&c)), "{}", fb2.book.title);
        assert!(!read(&samples().join("sample-utf8.fb2.zip")).unwrap().book.title.is_empty());
    }

    #[test]
    fn check_refuses_a_module_that_is_off() {
        let mut settings = Map::new();
        settings.insert("disabledModules".into(), serde_json::json!(["format:fb2"]));
        let state = store::State { settings, ..store::State::default() };
        let epub = by_module("format:epub").unwrap();
        let fb2 = by_module("format:fb2").unwrap();
        assert_eq!(check(&state, epub), Ok(()));
        assert_eq!(check(&state, fb2), Err("FB2 is turned off in Settings > Modules".into()));
        assert!(modules::is_off(&state, "format:fb2"));
    }
}
