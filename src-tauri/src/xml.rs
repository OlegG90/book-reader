//! A small event walk over XML documents (EPUB OPF, FB2, OPDS feeds), with
//! namespaces dropped: elements and attributes are matched by local name.

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::io;

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

pub enum Xml<'a> {
    /// An element starts; the stack holds the local names of its ancestors.
    Start(&'a [String], &'a BytesStart<'a>),
    /// Text inside the innermost element of the stack, entities resolved.
    Text(&'a [String], &'a str),
    /// An element ends; the stack holds its ancestors, as at its start.
    End(&'a [String], &'a str),
}

/// Walks an XML document, reporting element starts and text.
pub fn walk_xml(xml: &str, mut on: impl FnMut(Xml)) -> io::Result<()> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    loop {
        match reader.read_event().map_err(invalid)? {
            Event::Start(e) => {
                on(Xml::Start(&stack, &e));
                stack.push(local_name(&e));
            }
            Event::Empty(e) => on(Xml::Start(&stack, &e)),
            Event::End(_) => {
                if let Some(name) = stack.pop() {
                    on(Xml::End(&stack, &name));
                }
            }
            Event::Text(t) => on(Xml::Text(&stack, &t.decode().map_err(invalid)?)),
            Event::CData(t) => on(Xml::Text(&stack, &t.decode().map_err(invalid)?)),
            Event::GeneralRef(r) => {
                let named = r.decode().map_err(invalid)?;
                let c = r.resolve_char_ref().ok().flatten().or(match named.as_ref() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => None,
                });
                if let Some(c) = c {
                    on(Xml::Text(&stack, c.encode_utf8(&mut [0; 4])));
                }
            }
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

pub fn local_name(e: &BytesStart) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

/// An attribute by local name (any namespace prefix).
pub fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name.as_bytes())
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// The element's parent and own name, from a text event's stack.
pub fn last_two(stack: &[String]) -> (Option<&str>, Option<&str>) {
    let parent = stack.len().checked_sub(2).map(|i| stack[i].as_str());
    (parent, stack.last().map(String::as_str))
}
