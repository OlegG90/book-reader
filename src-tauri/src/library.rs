//! The library folder: finding the books in it and caching their metadata
//! and covers, so a rescan only reads files that are new or changed.

use crate::fingerprint::fingerprint;
use crate::metadata::{self, Author, Format};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A book in the library, as cached between scans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// Relative to the library folder, with `/` separators.
    pub path: String,
    pub size: u64,
    /// Modification time in ms since the Unix epoch.
    pub modified: u64,
    pub fingerprint: String,
    pub title: String,
    pub authors: Vec<Author>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    /// The cached cover's file name in the covers folder.
    pub cover: Option<String>,
}

impl Entry {
    pub fn absolute(&self, root: &Path) -> PathBuf {
        root.join(&self.path)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    root: PathBuf,
    books: Vec<Entry>,
}

/// Where scans keep their results: `library.json` and a `covers` folder.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: PathBuf) -> Self {
        Cache { dir }
    }

    fn index(&self) -> PathBuf {
        self.dir.join("library.json")
    }

    pub fn covers(&self) -> PathBuf {
        self.dir.join("covers")
    }

    /// The books found by the last scan of `root` (empty for another folder).
    pub fn load(&self, root: &Path) -> Vec<Entry> {
        fs::read(self.index())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|c| c.root == root)
            .map(|c| c.books)
            .unwrap_or_default()
    }

    fn save(&self, root: &Path, books: &[Entry]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let file = CacheFile { root: root.to_path_buf(), books: books.to_vec() };
        crate::store::write_atomically(&self.index(), &serde_json::to_vec(&file)?)
    }
}

/// Lists the books under `root`, reusing cached metadata for files whose size
/// and modification time are unchanged, and saves the result to the cache.
pub fn scan(root: &Path, cache: &Cache) -> io::Result<Vec<Entry>> {
    let previous: HashMap<String, Entry> = cache.load(root).into_iter().map(|e| (e.path.clone(), e)).collect();
    let mut files = Vec::new();
    find_books(root, &mut files)?;

    let mut books: Vec<Entry> = files
        .into_iter()
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok()?;
            let rel = relative(root, &path)?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            match previous.get(&rel) {
                Some(e) if e.size == meta.len() && e.modified == modified => Some(e.clone()),
                _ => read_entry(&path, rel, meta.len(), modified, cache).ok(),
            }
        })
        .collect();
    books.sort_by(|a, b| a.path.cmp(&b.path));

    remove_unused_covers(cache, &books);
    cache.save(root, &books)?;
    Ok(books)
}

/// All book files under `dir`, skipping hidden folders.
fn find_books(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for item in fs::read_dir(dir)?.flatten() {
        let path = item.path();
        let Ok(kind) = item.file_type() else { continue };
        if kind.is_dir() {
            if !item.file_name().to_string_lossy().starts_with('.') {
                // An unreadable subfolder shouldn't hide the rest of the library.
                let _ = find_books(&path, out);
            }
        } else if kind.is_file() && Format::of(&path).is_some() {
            out.push(path);
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators.
pub fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(parts.join("/"))
}

/// Reads a new or changed file. A book whose metadata can't be read still
/// appears, titled after its file name.
fn read_entry(path: &Path, rel: String, size: u64, modified: u64, cache: &Cache) -> io::Result<Entry> {
    let fingerprint = fingerprint(path)?;
    let meta = metadata::read(path).unwrap_or_default();
    let title = if meta.title.is_empty() { title_from_file_name(path) } else { meta.title };
    let cover = meta.cover.and_then(|cover| {
        let name = format!("{fingerprint}.{}", cover.ext);
        fs::create_dir_all(cache.covers()).ok()?;
        fs::write(cache.covers().join(&name), &cover.bytes).ok()?;
        Some(name)
    });
    Ok(Entry {
        path: rel,
        size,
        modified,
        fingerprint,
        title,
        authors: meta.authors,
        series: meta.series,
        series_index: meta.series_index,
        cover,
    })
}

/// "Some Book.fb2.zip" -> "Some Book".
pub fn title_from_file_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let lower = name.to_lowercase();
    let cut = [".fb2.zip", ".epub", ".fb2", ".fbz"].iter().find(|ext| lower.ends_with(*ext)).map_or(0, |ext| ext.len());
    name[..name.len() - cut].to_string()
}

fn remove_unused_covers(cache: &Cache, books: &[Entry]) {
    let used: HashSet<&str> = books.iter().filter_map(|b| b.cover.as_deref()).collect();
    let Ok(items) = fs::read_dir(cache.covers()) else { return };
    for item in items.flatten() {
        if !used.contains(item.file_name().to_string_lossy().as_ref()) {
            let _ = fs::remove_file(item.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../samples")
    }

    fn library_with_samples() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        fs::create_dir_all(root.join("Carroll Lewis")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::copy(samples().join("alice-in-wonderland.epub"), root.join("Carroll Lewis/Alice.epub")).unwrap();
        fs::copy(samples().join("sample-utf8.fb2.zip"), root.join("sample.fb2.zip")).unwrap();
        fs::copy(samples().join("sample-utf8.fb2.zip"), root.join(".hidden/skip.fb2.zip")).unwrap();
        fs::write(root.join("notes.txt"), "not a book").unwrap();
        (dir, root)
    }

    #[test]
    fn finds_books_and_reads_their_metadata() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let books = scan(&root, &cache).unwrap();
        let paths: Vec<_> = books.iter().map(|b| b.path.as_str()).collect();
        assert_eq!(paths, ["Carroll Lewis/Alice.epub", "sample.fb2.zip"]);
        assert!(books[0].title.contains("Alice"));
        let cover = books[0].cover.as_ref().expect("the sample EPUB has a cover");
        assert!(cache.covers().join(cover).is_file());
    }

    #[test]
    fn rescans_reuse_the_cache_and_drop_removed_books() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let first = scan(&root, &cache).unwrap();
        assert_eq!(cache.load(&root), first);

        // A cached entry is trusted while size and time match: mark it to prove reuse.
        let mut marked = first.clone();
        marked[1].title = "From cache".into();
        cache.save(&root, &marked).unwrap();
        assert_eq!(scan(&root, &cache).unwrap()[1].title, "From cache");

        fs::remove_file(root.join("Carroll Lewis/Alice.epub")).unwrap();
        let after = scan(&root, &cache).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(fs::read_dir(cache.covers()).unwrap().count(), 0, "the removed book's cover is deleted");
    }

    #[test]
    fn a_cache_for_another_folder_is_ignored() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        scan(&root, &cache).unwrap();
        assert!(cache.load(&dir.path().join("elsewhere")).is_empty());
    }

    #[test]
    fn unreadable_books_are_titled_after_the_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Broken Book.epub"), "not a zip").unwrap();
        let books = scan(dir.path(), &Cache::new(dir.path().join(".cache"))).unwrap();
        assert_eq!(books[0].title, "Broken Book");
        assert!(books[0].authors.is_empty());
    }

    #[test]
    fn titles_from_file_names_drop_the_book_extension() {
        assert_eq!(title_from_file_name(Path::new("a/Some Book.FB2.zip")), "Some Book");
        assert_eq!(title_from_file_name(Path::new("x.epub")), "x");
        assert_eq!(title_from_file_name(Path::new("v1.2 notes.fbz")), "v1.2 notes");
    }
}
