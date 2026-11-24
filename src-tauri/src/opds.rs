//! OPDS catalogs (Calibre-Web-Automated, the Calibre Content Server): reading
//! their Atom feeds and OpenSearch descriptions. Fetching is elsewhere; this
//! only turns text into what the catalog screen shows.

use crate::formats;
use crate::xml::{attr, invalid, last_two, local_name, walk_xml, Xml};
use serde::Serialize;
use std::io;
use url::Url;

/// One page of a catalog: folders to open and books to download.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub title: String,
    pub entries: Vec<Entry>,
    /// The next page of a long list.
    pub next: Option<String>,
    /// The OpenSearch description, for searching the catalog.
    pub search: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Entry {
    /// Another feed: "Authors", "New books", an author's books...
    Folder { title: String, href: String, summary: String },
    Book(Book),
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub summary: String,
    pub cover: Option<String>,
    /// The files the server offers, as it lists them.
    pub files: Vec<BookFile>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookFile {
    pub href: String,
    pub format: FileFormat,
}

/// The formats the reader opens; anything else the server offers is left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileFormat {
    Epub,
    Fb2,
    Fb2Zip,
}

impl FileFormat {
    /// The file extension the library uses for it.
    pub fn extension(self) -> &'static str {
        match self {
            FileFormat::Epub => "epub",
            FileFormat::Fb2 => "fb2",
            FileFormat::Fb2Zip => "fb2.zip",
        }
    }

    /// The module the format belongs to (`format:epub`, `format:fb2`).
    pub fn module(self) -> &'static str {
        match self {
            FileFormat::Epub => "format:epub",
            FileFormat::Fb2 | FileFormat::Fb2Zip => "format:fb2",
        }
    }

    /// From a link's media type or, failing that, its file extension (the registry knows the endings).
    fn of(media_type: &str, href: &str) -> Option<FileFormat> {
        let media_type = media_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        match media_type.as_str() {
            "application/epub+zip" => return Some(FileFormat::Epub),
            "application/x-fictionbook+xml" | "application/fb2+xml" | "text/fb2+xml" | "application/x-fictionbook" => {
                return Some(FileFormat::Fb2)
            }
            "application/fb2+zip" | "application/x-zip-compressed-fb2" => return Some(FileFormat::Fb2Zip),
            _ => {}
        }
        let path = href.split(['?', '#']).next().unwrap_or("").trim_end_matches('/').to_ascii_lowercase();
        if let Some((_, reader)) = formats::split(&path) {
            return Some(match reader.module() {
                "format:epub" => FileFormat::Epub,
                _ if reader.extension_for(&path) == Some("fb2") => FileFormat::Fb2,
                _ => FileFormat::Fb2Zip,
            });
        }
        // Calibre-Web-Automated names downloads after the format (`/fb2`).
        if path.ends_with("/epub") {
            Some(FileFormat::Epub)
        } else if path.ends_with("/fb2.zip") {
            Some(FileFormat::Fb2Zip)
        } else if path.ends_with("/fb2") {
            Some(FileFormat::Fb2)
        } else {
            None
        }
    }
}

/// `href` made absolute against the feed's own address.
fn resolve(base: &Url, href: &str) -> Option<String> {
    base.join(href.trim()).ok().map(String::from)
}

/// An entry while its elements are being read.
#[derive(Default)]
struct Draft {
    id: String,
    title: String,
    authors: Vec<String>,
    summary: String,
    cover: Option<String>,
    thumbnail: Option<String>,
    files: Vec<BookFile>,
    /// A link to another feed (a folder entry).
    feed: Option<String>,
}

impl Draft {
    fn finish(self) -> Option<Entry> {
        let title = self.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let summary = self.summary.trim().to_string();
        if !self.files.is_empty() {
            return Some(Entry::Book(Book {
                id: self.id.trim().to_string(),
                title,
                authors: self.authors.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect(),
                summary,
                // The thumbnail is smaller, and enough for a list.
                cover: self.thumbnail.or(self.cover),
                files: self.files,
            }));
        }
        self.feed.map(|href| Entry::Folder { title, href, summary })
    }
}

/// Reads an OPDS feed fetched from `url`; links come back absolute.
pub fn parse_feed(xml: &str, url: &str) -> io::Result<Feed> {
    let base = Url::parse(url).map_err(invalid)?;
    let mut feed = Feed::default();
    let mut draft: Option<Draft> = None;
    let mut is_feed = false;

    walk_xml(xml, |event| match event {
        Xml::Start(stack, e) => {
            let name = local_name(e);
            if stack.is_empty() {
                is_feed = name == "feed";
                return;
            }
            match (stack.len(), name.as_str()) {
                (1, "entry") => draft = Some(Draft::default()),
                (1, "link") => {
                    let rel = attr(e, "rel").unwrap_or_default();
                    let href = attr(e, "href").and_then(|h| resolve(&base, &h));
                    match rel.as_str() {
                        "next" => feed.next = href,
                        "search" => feed.search = href,
                        _ => {}
                    }
                }
                (2, "link") => {
                    let (Some(d), Some(href)) = (draft.as_mut(), attr(e, "href").and_then(|h| resolve(&base, &h))) else { return };
                    let rel = attr(e, "rel").unwrap_or_default();
                    let media_type = attr(e, "type").unwrap_or_default();
                    if rel.starts_with("http://opds-spec.org/acquisition") {
                        if let Some(format) = FileFormat::of(&media_type, &href) {
                            d.files.push(BookFile { href, format });
                        }
                    } else if rel == "http://opds-spec.org/image/thumbnail" || rel == "http://opds-spec.org/thumbnail" {
                        d.thumbnail = Some(href);
                    } else if rel == "http://opds-spec.org/image" || rel == "http://opds-spec.org/cover" {
                        d.cover = Some(href);
                    } else if media_type.starts_with("application/atom+xml") && d.feed.is_none() {
                        d.feed = Some(href);
                    }
                }
                _ => {}
            }
        }
        Xml::Text(stack, text) => match (stack.len(), last_two(stack)) {
            (2, (Some("feed"), Some("title"))) => feed.title.push_str(text),
            (3, (Some("entry"), Some("title"))) => draft.iter_mut().for_each(|d| d.title.push_str(text)),
            (3, (Some("entry"), Some("id"))) => draft.iter_mut().for_each(|d| d.id.push_str(text)),
            (3, (Some("entry"), Some("summary" | "content"))) => draft.iter_mut().for_each(|d| d.summary.push_str(text)),
            (4, (Some("author"), Some("name"))) => {
                if let Some(d) = draft.as_mut() {
                    d.authors.push(text.to_string());
                }
            }
            _ => {}
        },
        Xml::End(stack, name) => {
            if stack.len() == 1 && name == "entry" {
                if let Some(entry) = draft.take().and_then(Draft::finish) {
                    feed.entries.push(entry);
                }
            }
        }
    })?;
    if !is_feed {
        return Err(invalid("not an OPDS feed"));
    }
    feed.title = feed.title.trim().to_string();
    Ok(feed)
}

/// The search URL template from an OpenSearch description, with
/// `{searchTerms}` where the query goes.
pub fn parse_search_template(xml: &str, url: &str) -> io::Result<String> {
    let base = Url::parse(url).map_err(invalid)?;
    let mut atom = None;
    let mut any = None;
    walk_xml(xml, |event| {
        if let Xml::Start(_, e) = event {
            if local_name(e) != "Url" {
                return;
            }
            let Some(template) = attr(e, "template") else { return };
            if attr(e, "type").is_some_and(|t| t.starts_with("application/atom+xml")) {
                atom.get_or_insert(template.clone());
            }
            any.get_or_insert(template);
        }
    })?;
    let template = atom.or(any).ok_or_else(|| invalid("no search URL in the OpenSearch description"))?;
    // `join` would percent-encode the braces, so resolve around the placeholder.
    let (before, after) = template.split_once("{searchTerms}").ok_or_else(|| invalid("the search URL has no {searchTerms}"))?;
    let resolved = base.join(before).map_err(invalid)?;
    Ok(format!("{resolved}{{searchTerms}}{after}"))
}

/// A search URL for `query` from a template with `{searchTerms}`.
pub fn search_url(template: &str, query: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    template.replace("{searchTerms}", &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog">
  <id>urn:uuid:root</id>
  <title>Calibre-Web</title>
  <link rel="search" type="application/opensearchdescription+xml" href="/opds/osd"/>
  <link rel="next" type="application/atom+xml;profile=opds-catalog" href="/opds/books?offset=60"/>
  <entry>
    <title>Authors</title>
    <id>calibre-web:authors</id>
    <link rel="subsection" type="application/atom+xml;profile=opds-catalog" href="/opds/author"/>
    <content type="text">Books ordered by Author</content>
  </entry>
  <entry>
    <title>Захар  Беркут</title>
    <id>urn:uuid:42</id>
    <author><name>Іван Франко</name></author>
    <author><name>Second Author</name></author>
    <summary>A &amp; B</summary>
    <link rel="http://opds-spec.org/image" type="image/jpeg" href="/opds/cover/42"/>
    <link rel="http://opds-spec.org/image/thumbnail" type="image/jpeg" href="/opds/cover_240_240/42"/>
    <link rel="http://opds-spec.org/acquisition" type="application/pdf" href="/opds/download/42/pdf/"/>
    <link rel="http://opds-spec.org/acquisition" type="application/x-fictionbook+xml" href="/opds/download/42/fb2/"/>
    <link rel="http://opds-spec.org/acquisition" type="application/epub+zip" href="/opds/download/42/epub/"/>
  </entry>
  <entry>
    <title>Only PDF</title>
    <id>urn:uuid:43</id>
    <link rel="http://opds-spec.org/acquisition" type="application/pdf" href="/opds/download/43/pdf/"/>
  </entry>
</feed>"#;

    #[test]
    fn reads_folders_books_and_feed_links() {
        let feed = parse_feed(ROOT, "https://books.example/opds").unwrap();
        assert_eq!(feed.title, "Calibre-Web");
        assert_eq!(feed.search.as_deref(), Some("https://books.example/opds/osd"));
        assert_eq!(feed.next.as_deref(), Some("https://books.example/opds/books?offset=60"));
        assert_eq!(feed.entries.len(), 2, "a book with no readable format is left out");

        let Entry::Folder { title, href, summary } = &feed.entries[0] else { panic!("{:?}", feed.entries[0]) };
        assert_eq!((title.as_str(), href.as_str(), summary.as_str()), ("Authors", "https://books.example/opds/author", "Books ordered by Author"));

        let Entry::Book(book) = &feed.entries[1] else { panic!("{:?}", feed.entries[1]) };
        assert_eq!(book.title, "Захар Беркут");
        assert_eq!(book.authors, ["Іван Франко", "Second Author"]);
        assert_eq!(book.summary, "A & B");
        assert_eq!(book.cover.as_deref(), Some("https://books.example/opds/cover_240_240/42"));
        assert_eq!(book.files.len(), 2);
        assert_eq!(book.files[1].href, "https://books.example/opds/download/42/epub/");
    }

    #[test]
    fn file_formats_belong_to_modules() {
        assert_eq!(FileFormat::Epub.module(), "format:epub");
        assert_eq!(FileFormat::Fb2.module(), "format:fb2");
        assert_eq!(FileFormat::Fb2Zip.module(), "format:fb2");
    }

    #[test]
    fn formats_from_media_types_or_links() {
        assert_eq!(FileFormat::of("application/epub+zip", "x"), Some(FileFormat::Epub));
        assert_eq!(FileFormat::of("application/x-fictionbook+xml; charset=utf-8", "x"), Some(FileFormat::Fb2));
        assert_eq!(FileFormat::of("application/octet-stream", "/get/fb2.zip/7"), None);
        assert_eq!(FileFormat::of("application/octet-stream", "/books/7.fb2.zip"), Some(FileFormat::Fb2Zip));
        assert_eq!(FileFormat::of("", "/opds/download/7/fb2/"), Some(FileFormat::Fb2));
        // CWA serves FB2 as text/xml: the link says which format it is.
        assert_eq!(FileFormat::of("text/xml", "/opds/download/7/fb2/"), Some(FileFormat::Fb2));
        assert_eq!(FileFormat::of("application/pdf", "/x.pdf"), None);
    }

    #[test]
    fn rejects_what_is_not_a_feed() {
        assert!(parse_feed("<html><body>Login</body></html>", "https://x/opds").is_err());
    }

    #[test]
    fn search_templates() {
        let osd = r#"<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
            <Url type="text/html" template="/search?q={searchTerms}"/>
            <Url type="application/atom+xml" template="/opds/search/{searchTerms}"/>
          </OpenSearchDescription>"#;
        let template = parse_search_template(osd, "https://books.example/opds/osd").unwrap();
        assert_eq!(template, "https://books.example/opds/search/{searchTerms}");
        assert_eq!(search_url(&template, "Франко & co"), "https://books.example/opds/search/%D0%A4%D1%80%D0%B0%D0%BD%D0%BA%D0%BE+%26+co");
    }
}
