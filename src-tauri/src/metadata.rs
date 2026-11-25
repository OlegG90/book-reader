//! Book metadata for the library: title, authors, series and cover, read
//! straight from EPUB (OPF) and FB2 (`title-info`) files.

use base64::Engine;
use serde::{Deserialize, Serialize};
use crate::xml::{attr, invalid, last_two, local_name, walk_xml, Xml};
use std::collections::HashMap;
use std::io::{self, Read, Seek};
use std::path::Path;
use zip::ZipArchive;

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Author {
    pub first: String,
    pub last: String,
}

impl Author {
    /// "First Last", for showing.
    pub fn display(&self) -> String {
        join_nonempty(&[&self.first, &self.last])
    }

    /// "Last First", for sorting and folder names.
    pub fn sort_name(&self) -> String {
        join_nonempty(&[&self.last, &self.first])
    }

    /// Splits a free-form name: "Last, First" (OPF file-as) or "First Middle Last".
    pub fn parse(name: &str) -> Author {
        let name = clean(name);
        if let Some((last, first)) = name.split_once(',') {
            return Author { first: first.trim().into(), last: last.trim().into() };
        }
        match name.rsplit_once(' ') {
            Some((first, last)) => Author { first: first.into(), last: last.into() },
            None => Author { first: String::new(), last: name },
        }
    }
}

fn join_nonempty(parts: &[&str]) -> String {
    parts.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Collapses runs of whitespace and trims.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Appends a text chunk from nested markup (an annotation, a description):
/// words stay apart, punctuation hugs the previous word.
fn push_text(acc: &mut String, chunk: &str) {
    let chunk = chunk.trim();
    if chunk.is_empty() {
        return;
    }
    let starts_with_punct =
        chunk.starts_with(['.', ',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '”', '’', '…']);
    if !acc.is_empty() && !acc.ends_with(char::is_whitespace) && !starts_with_punct {
        acc.push(' ');
    }
    acc.push_str(chunk);
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    pub bytes: Vec<u8>,
    /// File extension for the cached image ("jpg", "png", ...).
    pub ext: String,
}

/// What the library and its naming pattern know about a book.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub title: String,
    pub authors: Vec<Author>,
    pub series: Option<String>,
    /// As written in the book ("3", "2.5").
    pub series_index: Option<String>,
    /// The ISO language code the file states ("en", "uk").
    #[serde(default)]
    pub language: Option<String>,
    /// The book's own blurb, when the file carries one.
    #[serde(default)]
    pub annotation: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Metadata {
    pub book: Book,
    pub cover: Option<Cover>,
}

pub(crate) fn zip_error(e: zip::result::ZipError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

pub(crate) fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> io::Result<Vec<u8>> {
    let mut entry = zip.by_name(name).map_err(zip_error)?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

// ---- FB2 --------------------------------------------------------------------

/// Decodes an FB2 file using the encoding in its XML declaration (UTF-8 if none).
fn decode_fb2(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).into_owned();
    let label = head
        .split_once("encoding=")
        .and_then(|(_, rest)| {
            let quote = rest.chars().next()?;
            rest[quote.len_utf8()..].split(quote).next().map(str::to_string)
        })
        .unwrap_or_default();
    let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// The path below `<FictionBook><description><title-info>`, if inside it.
fn in_title_info(stack: &[String]) -> Option<&[String]> {
    (stack.len() >= 3 && stack[1] == "description" && stack[2] == "title-info").then(|| &stack[3..])
}

pub(crate) fn fb2(bytes: &[u8]) -> io::Result<Metadata> {
    let xml = decode_fb2(bytes);
    let mut meta = Metadata::default();
    let mut annotation = String::new();
    let mut langs: Vec<String> = Vec::new();
    let mut cover_id: Option<String> = None;
    // id -> (content type, base64 text)
    let mut binaries: HashMap<String, (String, String)> = HashMap::new();
    let mut binary_id = String::new();

    walk_xml(&xml, |event| match event {
        Xml::Start(stack, e) => {
            let name = local_name(e);
            if stack.len() == 1 && name == "binary" {
                binary_id = attr(e, "id").unwrap_or_default();
                binaries.insert(binary_id.clone(), (attr(e, "content-type").unwrap_or_default(), String::new()));
                return;
            }
            let Some(path) = in_title_info(stack) else { return };
            match (path, name.as_str()) {
                ([], "author") => meta.book.authors.push(Author::default()),
                ([], "lang") => langs.push(String::new()),
                ([], "sequence") if meta.book.series.is_none() => {
                    meta.book.series = attr(e, "name").map(|s| clean(&s)).filter(|s| !s.is_empty());
                    meta.book.series_index = attr(e, "number").map(|s| clean(&s)).filter(|s| !s.is_empty() && s != "0");
                }
                ([p], "image") if p == "coverpage" => {
                    cover_id = attr(e, "href").map(|h| h.trim_start_matches('#').to_string());
                }
                _ => {}
            }
        }
        Xml::Text(stack, text) => {
            if stack.len() == 2 && stack[1] == "binary" {
                if let Some(entry) = binaries.get_mut(&binary_id) {
                    entry.1.push_str(text);
                }
                return;
            }
            let Some(path) = in_title_info(stack) else { return };
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            match path.as_slice() {
                ["book-title"] => meta.book.title.push_str(text),
                ["lang"] => {
                    langs.last_mut().into_iter().for_each(|l| l.push_str(text));
                }
                ["annotation", ..] => push_text(&mut annotation, text),
                ["author", part] => {
                    let Some(a) = meta.book.authors.last_mut() else { return };
                    match *part {
                        "first-name" | "middle-name" => a.first = join_nonempty(&[&a.first, &clean(text)]),
                        "last-name" => a.last.push_str(&clean(text)),
                        "nickname" if a.first.is_empty() && a.last.is_empty() => a.last = clean(text),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Xml::End(..) => {}
    })?;

    meta.book.authors.retain(|a| !a.first.is_empty() || !a.last.is_empty());
    meta.book.title = clean(&meta.book.title);
    meta.book.language = langs.into_iter().map(|l| clean(&l)).find(|l| !l.is_empty());
    meta.book.annotation = Some(clean(&annotation)).filter(|s| !s.is_empty());
    if let Some((content_type, data)) = cover_id.and_then(|id| binaries.remove(&id)) {
        let data: String = data.split_whitespace().collect();
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            meta.cover = Some(Cover { ext: image_ext(&content_type, ""), bytes });
        }
    }
    Ok(meta)
}

/// An image extension from a media type or, failing that, a file name.
fn image_ext(media_type: &str, href: &str) -> String {
    match media_type.to_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg".into(),
        "image/png" => "png".into(),
        "image/gif" => "gif".into(),
        "image/webp" => "webp".into(),
        "image/svg+xml" => "svg".into(),
        _ => Path::new(href)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|e| ["jpg", "jpeg", "png", "gif", "webp", "svg"].contains(&e.as_str()))
            .unwrap_or_else(|| "jpg".into()),
    }
}

// ---- EPUB -------------------------------------------------------------------

struct ManifestItem {
    href: String,
    media_type: String,
}

struct Creator {
    role: Option<String>,
    file_as: Option<String>,
    name: String,
}

/// Whether the text sits anywhere under the OPF description.
fn in_metadata_description(stack: &[String]) -> bool {
    stack.windows(2).any(|w| w[0] == "metadata" && w[1] == "description")
}

pub(crate) fn epub<R: Read + Seek>(file: R) -> io::Result<Metadata> {
    let mut zip = ZipArchive::new(file).map_err(zip_error)?;
    let container = String::from_utf8_lossy(&read_entry(&mut zip, "META-INF/container.xml")?).into_owned();
    let mut opf_path = None;
    walk_xml(&container, |event| {
        if let Xml::Start(_, e) = event {
            if opf_path.is_none() && local_name(e) == "rootfile" {
                opf_path = attr(e, "full-path");
            }
        }
    })?;
    let opf_path = opf_path.ok_or_else(|| invalid("no OPF file"))?;
    let opf = String::from_utf8_lossy(&read_entry(&mut zip, &opf_path)?).into_owned();

    let mut meta = Metadata::default();
    let mut titles: Vec<String> = Vec::new();
    let mut creators: Vec<Creator> = Vec::new();
    let mut languages: Vec<String> = Vec::new();
    let mut description = String::new();
    let mut collection: Option<String> = None;
    let mut in_collection = false;
    let mut cover_id = None;
    let mut cover_item = None;
    let mut manifest: HashMap<String, ManifestItem> = HashMap::new();

    walk_xml(&opf, |event| match event {
        Xml::Start(stack, e) => {
            let name = local_name(e);
            match (stack.last().map(String::as_str), name.as_str()) {
                (Some("metadata"), "title") => titles.push(String::new()),
                (Some("metadata"), "language") => languages.push(String::new()),
                (Some("metadata"), "creator") => {
                    creators.push(Creator { role: attr(e, "role"), file_as: attr(e, "file-as"), name: String::new() })
                }
                (Some("metadata"), "meta") => {
                    match (attr(e, "name").as_deref(), attr(e, "content")) {
                        (Some("calibre:series"), Some(v)) => meta.book.series = Some(clean(&v)).filter(|s| !s.is_empty()),
                        (Some("calibre:series_index"), Some(v)) => meta.book.series_index = Some(series_number(&v)),
                        (Some("cover"), Some(v)) => cover_id = Some(v),
                        _ => {}
                    }
                    in_collection = attr(e, "property").as_deref() == Some("belongs-to-collection") && attr(e, "refines").is_none();
                }
                (Some("manifest"), "item") => {
                    if let (Some(id), Some(href)) = (attr(e, "id"), attr(e, "href")) {
                        if attr(e, "properties").is_some_and(|p| p.split_whitespace().any(|p| p == "cover-image")) {
                            cover_item = Some(id.clone());
                        }
                        manifest.insert(id, ManifestItem { href, media_type: attr(e, "media-type").unwrap_or_default() });
                    }
                }
                _ => {}
            }
        }
        Xml::Text(stack, text) => match last_two(stack) {
            (Some("metadata"), Some("title")) => titles.last_mut().into_iter().for_each(|t| t.push_str(text)),
            (Some("metadata"), Some("language")) => languages.last_mut().into_iter().for_each(|l| l.push_str(text)),
            _ if in_metadata_description(stack) => push_text(&mut description, text),
            (Some("metadata"), Some("creator")) => creators.last_mut().into_iter().for_each(|c| c.name.push_str(text)),
            (Some("metadata"), Some("meta")) if in_collection => collection.get_or_insert_with(String::new).push_str(text),
            _ => {}
        },
        Xml::End(..) => {}
    })?;

    meta.book.title = titles.first().map(|t| clean(t)).unwrap_or_default();
    meta.book.language = languages.into_iter().map(|l| clean(&l)).find(|l| !l.is_empty());
    meta.book.annotation = Some(clean(&description)).filter(|s| !s.is_empty());
    meta.book.authors = creators
        .iter()
        .filter(|c| c.role.as_deref().is_none_or(|r| r == "aut") && !c.name.trim().is_empty())
        .map(|c| match &c.file_as {
            Some(f) if f.contains(',') => Author::parse(f),
            _ => Author::parse(&c.name),
        })
        .collect();
    if meta.book.series.is_none() {
        meta.book.series = collection.map(|c| clean(&c)).filter(|c| !c.is_empty());
    }

    if let Some(item) = cover_item.or(cover_id).and_then(|id| manifest.remove(&id)) {
        if let Ok(bytes) = read_entry(&mut zip, &resolve_href(&opf_path, &item.href)) {
            meta.cover = Some(Cover { ext: image_ext(&item.media_type, &item.href), bytes });
        }
    }
    Ok(meta)
}

/// Calibre writes series indexes as floats ("3.0"); show whole numbers plainly.
fn series_number(value: &str) -> String {
    let value = value.trim();
    match value.parse::<f64>() {
        Ok(n) if n.fract() == 0.0 => format!("{}", n as i64),
        _ => value.to_string(),
    }
}

/// Resolves a manifest href against the OPF file's folder, inside the zip.
fn resolve_href(opf_path: &str, href: &str) -> String {
    let href = percent_decode(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> = opf_path.rsplit_once('/').map(|(dir, _)| dir.split('/').collect()).unwrap_or_default();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    #[test]
    fn authors_parse_both_name_orders() {
        assert_eq!(Author::parse("Carroll, Lewis"), Author { first: "Lewis".into(), last: "Carroll".into() });
        assert_eq!(Author::parse("Jane  Austen"), Author { first: "Jane".into(), last: "Austen".into() });
        assert_eq!(Author::parse("Homer"), Author { first: String::new(), last: "Homer".into() });
        assert_eq!(Author::parse("Arthur Conan Doyle").sort_name(), "Doyle Arthur Conan");
    }

    const FB2: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
 <description><title-info>
  <author><first-name>Іван</first-name><middle-name>Якович</middle-name><last-name>Франко</last-name></author>
  <author><nickname>Anon</nickname></author>
  <book-title>Захар  Беркут &amp; інші</book-title>
  <coverpage><image l:href="#c.png"/></coverpage>
  <sequence name="Повісті" number="2"/>
 </title-info>
 <document-info><author><last-name>Editor</last-name></author></document-info></description>
 <body><section><p>Text</p></section></body>
 <binary id="c.png" content-type="image/png">iVBORw0K
 GgoAAAA=</binary>
</FictionBook>"##;

    #[test]
    fn fb2_title_info_authors_series_and_cover() {
        let meta = fb2(FB2.as_bytes()).unwrap();
        assert_eq!(meta.book.title, "Захар Беркут & інші");
        assert_eq!(meta.book.authors.len(), 2);
        assert_eq!(meta.book.authors[0].sort_name(), "Франко Іван Якович");
        assert_eq!(meta.book.authors[1].display(), "Anon");
        assert_eq!(meta.book.series.as_deref(), Some("Повісті"));
        assert_eq!(meta.book.series_index.as_deref(), Some("2"));
        let cover = meta.cover.unwrap();
        assert_eq!(cover.ext, "png");
        assert_eq!(&cover.bytes[..4], b"\x89PNG");
    }

    #[test]
    fn fb2_encoding_comes_from_the_declaration() {
        let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode("<?xml version='1.0' encoding='windows-1251'?><a>Привіт</a>");
        assert!(decode_fb2(&bytes).contains("Привіт"));
    }

    #[test]
    fn fb2_lang_and_annotation() {
        let xml = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
 <description><title-info>
  <book-title>Book</book-title>
  <annotation><p>First paragraph.</p><p>Second <emphasis>one</emphasis>.</p></annotation>
  <lang>uk</lang>
 </title-info></description>
 <body><section><p>Text</p></section></body>
</FictionBook>"##;
        let meta = fb2(xml.as_bytes()).unwrap();
        assert_eq!(meta.book.language.as_deref(), Some("uk"));
        assert_eq!(meta.book.annotation.as_deref(), Some("First paragraph. Second one."));
    }

    #[test]
    fn books_without_lang_or_annotation_stay_empty() {
        let meta = fb2(FB2.as_bytes()).unwrap();
        assert_eq!(meta.book.language, None);
        assert_eq!(meta.book.annotation, None);
    }

    #[test]
    fn old_cached_books_load_without_the_new_fields() {
        // Caches written before language/annotation existed carry neither key.
        let book: Book = serde_json::from_value(serde_json::json!({
            "title": "Old",
            "authors": [],
            "series": null,
            "seriesIndex": null,
        }))
        .unwrap();
        assert_eq!(book.language, None);
        assert_eq!(book.annotation, None);
    }

    #[test]
    fn epub_language_and_description() {
        let opf = r#"<package xmlns:opf="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>Book</dc:title>
            <dc:language>en</dc:language>
            <dc:description>A tale <b>of two</b> cities.</dc:description>
          </metadata><manifest></manifest></package>"#;
        let meta = epub(Cursor::new(make_epub(opf, &[]))).unwrap();
        assert_eq!(meta.book.language.as_deref(), Some("en"));
        assert_eq!(meta.book.annotation.as_deref(), Some("A tale of two cities."));
    }

    fn make_epub(opf: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut out);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("META-INF/container.xml", options).unwrap();
        zip.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();
        zip.start_file("OEBPS/content.opf", options).unwrap();
        zip.write_all(opf.as_bytes()).unwrap();
        for (name, bytes) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn epub_calibre_series_file_as_and_cover() {
        let opf = r#"<package xmlns:opf="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>Book &amp; Title</dc:title>
            <dc:creator opf:role="aut" opf:file-as="Pratchett, Terry">Terry Pratchett</dc:creator>
            <dc:creator opf:role="ill">Someone Else</dc:creator>
            <meta name="calibre:series" content="Discworld"/>
            <meta name="calibre:series_index" content="3.0"/>
            <meta name="cover" content="cov"/>
          </metadata><manifest><item id="cov" href="images/my%20cover.jpg" media-type="image/jpeg"/></manifest></package>"#;
        let bytes = make_epub(opf, &[("OEBPS/images/my cover.jpg", b"JPEG")]);
        let meta = epub(Cursor::new(bytes)).unwrap();
        assert_eq!(meta.book.title, "Book & Title");
        assert_eq!(meta.book.authors, vec![Author { first: "Terry".into(), last: "Pratchett".into() }]);
        assert_eq!(meta.book.series.as_deref(), Some("Discworld"));
        assert_eq!(meta.book.series_index.as_deref(), Some("3"));
        assert_eq!(meta.cover, Some(Cover { bytes: b"JPEG".to_vec(), ext: "jpg".into() }));
    }

    #[test]
    fn epub3_collection_and_cover_image_property() {
        let opf = r##"<package><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:title>T</dc:title>
            <meta property="belongs-to-collection" id="c1">Saga</meta>
            <meta refines="#c1" property="group-position">1</meta>
          </metadata><manifest><item id="x" href="../c.png" media-type="image/png" properties="cover-image"/></manifest></package>"##;
        let bytes = make_epub(opf, &[("c.png", b"PNG")]);
        let meta = epub(Cursor::new(bytes)).unwrap();
        assert_eq!(meta.book.series.as_deref(), Some("Saga"));
        assert_eq!(meta.cover.unwrap().bytes, b"PNG");
    }

    #[test]
    fn href_resolution() {
        assert_eq!(resolve_href("OEBPS/content.opf", "img/a%20b.jpg"), "OEBPS/img/a b.jpg");
        assert_eq!(resolve_href("content.opf", "a.jpg"), "a.jpg");
        assert_eq!(resolve_href("a/b/content.opf", "../c.jpg#x"), "a/c.jpg");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
