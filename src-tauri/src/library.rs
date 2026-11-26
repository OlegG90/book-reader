//! The library folder: finding the books in it and caching their metadata
//! and covers, so a rescan only reads files that are new or changed.

use crate::fingerprint::fingerprint;
use crate::formats;
use crate::metadata::{self, Metadata};
use crate::naming;
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
    #[serde(flatten)]
    pub book: metadata::Book,
    /// The cached cover's file name in the covers folder.
    pub cover: Option<String>,
    /// When the book came into the library (ms since the Unix epoch). Copies
    /// keep the original's modification time, so that can't tell.
    #[serde(default)]
    pub added: u64,
}

impl Entry {
    pub fn absolute(&self, root: &Path) -> PathBuf {
        join(root, &self.path)
    }

    /// The reader for the entry's file; every scanned entry has one.
    pub fn reader(&self) -> Option<&'static dyn formats::FormatReader> {
        formats::split(&self.path).map(|(_, reader)| reader)
    }

    /// The entry's own file extension (`fb2.zip` for `x.FB2.ZIP`).
    pub fn extension(&self) -> Option<&'static str> {
        self.reader()?.extension_for(&self.path)
    }
}

/// A book file's fingerprint and metadata. A book whose metadata can't be
/// read is still a book, titled after its file name.
pub fn read_book(path: &Path) -> io::Result<(String, Metadata)> {
    let fingerprint = fingerprint(path)?;
    let mut meta = formats::read(path).unwrap_or_default();
    if meta.book.title.is_empty() {
        meta.book.title = title_from_file_name(path);
    }
    Ok((fingerprint, meta))
}

/// A library-relative path (`/` separators) under `root`.
pub fn join(root: &Path, relative: &str) -> PathBuf {
    relative.split('/').fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The moves that make every book follow the naming template: each book with
/// the new path it gets. A book whose identical copy already sits at its
/// template path stays where it is (nothing is ever overwritten or deleted).
pub fn organize_plan(root: &Path, books: &[Entry]) -> io::Result<Vec<(Entry, String)>> {
    // Targets of planned moves; files already on disk are checked there.
    let mut taken = HashSet::new();
    let mut plan = Vec::new();
    for book in books {
        let Some(extension) = book.extension() else { continue };
        let wanted = naming::template_path(&book.book, extension);
        if wanted.to_lowercase() == book.path.to_lowercase() {
            continue;
        }
        // The book's own file counts as the identical copy, so a book already
        // at "Title (2)" because "Title" is taken stays put.
        if let naming::Placement::Free(to) = naming::place(root, &wanted, &book.fingerprint, &taken)? {
            taken.insert(to.to_lowercase());
            plan.push((book.clone(), to));
        }
    }
    Ok(plan)
}

/// Bumped when `Entry` gains fields, so old caches rescan once instead of
/// serving stale metadata.
const CACHE_VERSION: u32 = 1;

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    version: u32,
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

    /// The books found by the last scan of `root` (empty for another folder,
    /// or for a cache written before the current entry shape).
    pub fn load(&self, root: &Path) -> Vec<Entry> {
        fs::read(self.index())
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|c| c.root == root && c.version == CACHE_VERSION)
            .map(|c| c.books)
            .unwrap_or_default()
    }

    fn save(&self, root: &Path, books: &[Entry]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let file = CacheFile { version: CACHE_VERSION, root: root.to_path_buf(), books: books.to_vec() };
        crate::store::write_atomically(&self.index(), &serde_json::to_vec(&file)?)
    }
}

/// Lists the books under `root`, reusing cached metadata for files whose size
/// and modification time are unchanged, and saves the result to the cache
/// when anything changed. Calls `progress` with the file count as it goes,
/// so the library screen can show the scan is moving. Every book file is
/// listed, also one whose format is turned off in Modules: it stays visible
/// (and keeps its added date) but refuses to open.
pub fn scan(root: &Path, cache: &Cache, progress: &mut dyn FnMut(usize)) -> io::Result<Vec<Entry>> {
    let cached = cache.load(root);
    let unchanged = |books: &[Entry]| books == cached.as_slice();
    // A book seen before (even under another name, after Organize) keeps its
    // date; on the very first scan the files' own dates are all there is.
    let first_scan = cached.is_empty();
    let added_before: HashMap<&str, u64> = cached.iter().map(|e| (e.fingerprint.as_str(), e.added)).collect();
    let previous: HashMap<&str, &Entry> = cached.iter().map(|e| (e.path.as_str(), e)).collect();
    let now = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let mut files = Vec::new();
    find_books(root, &mut files)?;

    let mut done = 0;
    let mut books: Vec<Entry> = files
        .into_iter()
        .filter_map(|path| {
            done += 1;
            progress(done);
            let meta = fs::metadata(&path).ok()?;
            let rel = relative(root, &path)?;
            let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
            match previous.get(rel.as_str()) {
                Some(e) if e.size == meta.len() && e.modified == modified => Some((*e).clone()),
                _ => {
                    let (fingerprint, meta_read) = read_book(&path).ok()?;
                    let added = match added_before.get(fingerprint.as_str()) {
                        Some(&added) => added,
                        None if first_scan => modified,
                        None => now,
                    };
                    let cover = cache_cover(cache, &fingerprint, meta_read.cover);
                    Some(Entry { path: rel, size: meta.len(), modified, fingerprint, book: meta_read.book, cover, added })
                }
            }
        })
        .collect();
    books.sort_by(|a, b| a.path.cmp(&b.path));

    if !unchanged(&books) {
        remove_unused_covers(cache, &books);
        cache.save(root, &books)?;
    }
    Ok(books)
}

/// All kept book files under `dir`, skipping hidden folders.
fn find_books(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for item in fs::read_dir(dir)?.flatten() {
        let path = item.path();
        let Ok(kind) = item.file_type() else { continue };
        if kind.is_dir() {
            if !item.file_name().to_string_lossy().starts_with('.') {
                // An unreadable subfolder shouldn't hide the rest of the library.
                let _ = find_books(&path, out);
            }
        } else if kind.is_file() && formats::of(&path).is_some() {
            out.push(path);
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    Some(parts.join("/"))
}

/// Saves a book's cover to the cache; returns its file name there.
fn cache_cover(cache: &Cache, fingerprint: &str, cover: Option<metadata::Cover>) -> Option<String> {
    let cover = cover?;
    let name = format!("{fingerprint}.{}", cover.ext);
    fs::create_dir_all(cache.covers()).ok()?;
    fs::write(cache.covers().join(&name), &cover.bytes).ok()?;
    Some(name)
}

/// "Some Book.fb2.zip" -> "Some Book".
fn title_from_file_name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    formats::split(&name).map_or(&*name, |(stem, _)| stem).to_string()
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
    fn old_caches_rescan_instead_of_serving_stale_metadata() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        // A cache from before the version field: same shape otherwise.
        std::fs::create_dir_all(cache.dir.join("covers")).unwrap();
        let old = serde_json::json!({ "root": root, "books": [] });
        std::fs::write(cache.index(), serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(cache.load(&root).is_empty(), "unknown versions rescan");
        let books = scan(&root, &cache, &mut |_| {}).unwrap();
        assert!(!books.is_empty());
        assert!(!cache.load(&root).is_empty(), "the rescan sticks");
    }

    #[test]
    fn finds_books_and_reads_their_metadata() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let books = scan(&root, &cache, &mut |_| {}).unwrap();
        let paths: Vec<_> = books.iter().map(|b| b.path.as_str()).collect();
        assert_eq!(paths, ["Carroll Lewis/Alice.epub", "sample.fb2.zip"]);
        assert!(books[0].book.title.contains("Alice"));
        let cover = books[0].cover.as_ref().expect("the sample EPUB has a cover");
        assert!(cache.covers().join(cover).is_file());
    }

    #[test]
    fn rescans_reuse_the_cache_and_drop_removed_books() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let first = scan(&root, &cache, &mut |_| {}).unwrap();
        assert_eq!(cache.load(&root), first);

        // A cached entry is trusted while size and time match: mark it to prove reuse.
        let mut marked = first.clone();
        marked[1].book.title = "From cache".into();
        cache.save(&root, &marked).unwrap();
        assert_eq!(scan(&root, &cache, &mut |_| {}).unwrap()[1].book.title, "From cache");

        fs::remove_file(root.join("Carroll Lewis/Alice.epub")).unwrap();
        let after = scan(&root, &cache, &mut |_| {}).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(fs::read_dir(cache.covers()).unwrap().count(), 0, "the removed book's cover is deleted");
    }

    #[test]
    fn a_cache_for_another_folder_is_ignored() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        scan(&root, &cache, &mut |_| {}).unwrap();
        assert!(cache.load(&dir.path().join("elsewhere")).is_empty());
    }

    #[test]
    fn unreadable_books_are_titled_after_the_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Broken Book.epub"), "not a zip").unwrap();
        let books = scan(dir.path(), &Cache::new(dir.path().join(".cache")), &mut |_| {}).unwrap();
        assert_eq!(books[0].book.title, "Broken Book");
        assert!(books[0].book.authors.is_empty());
    }

    #[test]
    fn organizing_moves_books_to_their_template_paths() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let books = scan(&root, &cache, &mut |_| {}).unwrap();
        let plan = organize_plan(&root, &books).unwrap();
        let moves: Vec<(&str, &str)> = plan.iter().map(|(b, to)| (b.path.as_str(), to.as_str())).collect();
        assert_eq!(moves.len(), 2);
        assert_eq!(moves[0].0, "Carroll Lewis/Alice.epub");
        assert!(moves[0].1.starts_with("Carroll Lewis/Alice"), "{}", moves[0].1);
        assert!(moves[1].1.ends_with(".fb2.zip"), "{}", moves[1].1);

        for (book, to) in &plan {
            naming::move_file(&book.absolute(&root), &join(&root, to)).unwrap();
        }
        let again = scan(&root, &cache, &mut |_| {}).unwrap();
        assert!(organize_plan(&root, &again).unwrap().is_empty(), "a second run changes nothing");
    }

    #[test]
    fn organizing_keeps_duplicates_where_they_are() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let books = scan(&root, &cache, &mut |_| {}).unwrap();
        let plan = organize_plan(&root, &books).unwrap();
        let (alice, to) = plan.iter().find(|(b, _)| b.path.ends_with("Alice.epub")).unwrap();
        // Put an identical copy where Alice should go: Alice itself then stays.
        fs::copy(alice.absolute(&root), join(&root, to)).unwrap();
        let books = scan(&root, &cache, &mut |_| {}).unwrap();
        let plan = organize_plan(&root, &books).unwrap();
        assert!(plan.iter().all(|(b, _)| !b.path.ends_with("Alice.epub")));
    }

    #[test]
    fn added_dates_survive_renames_and_new_books_get_today() {
        let (dir, root) = library_with_samples();
        let cache = Cache::new(dir.path().join("cache"));
        let first = scan(&root, &cache, &mut |_| {}).unwrap();
        assert!(first.iter().all(|b| b.added == b.modified), "the first scan uses file dates");

        let alice = &first[0];
        fs::rename(alice.absolute(&root), root.join("renamed.epub")).unwrap();
        fs::copy(samples().join("pride-and-prejudice.epub"), root.join("new.epub")).unwrap();
        let second = scan(&root, &cache, &mut |_| {}).unwrap();
        let find = |path: &str| second.iter().find(|b| b.path == path).unwrap();
        assert_eq!(find("renamed.epub").added, alice.added);
        assert!(find("new.epub").added > find("new.epub").modified, "a copy keeps its old file date");
    }

    #[test]
    fn titles_from_file_names_drop_the_book_extension() {
        assert_eq!(title_from_file_name(Path::new("a/Some Book.FB2.zip")), "Some Book");
        assert_eq!(title_from_file_name(Path::new("x.epub")), "x");
        assert_eq!(title_from_file_name(Path::new("v1.2 notes.fbz")), "v1.2 notes");
    }
}
