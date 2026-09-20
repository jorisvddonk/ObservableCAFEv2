//! Minimal RSS 2.0 / Atom feed parser.
//!
//! Deliberately tolerant and dependency-light: it walks the XML stream and
//! pulls out feed title and per-item title/link/date/summary. Namespaces are
//! ignored (local names only), and unknown elements are skipped. Both
//! `<item>` (RSS) and `<entry>` (Atom) are treated as items.

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;

/// One feed entry.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FeedItem {
    pub title: String,
    pub link: String,
    #[serde(default)]
    pub published: String,
    #[serde(default)]
    pub summary: String,
}

/// A parsed feed.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Feed {
    pub title: String,
    pub items: Vec<FeedItem>,
}

const FEED_TITLE: &str = "__feed_title__";

/// Elements whose text content is captured while inside an item.
fn is_captured(name: &str) -> bool {
    matches!(
        name,
        // `link` is text in RSS, an href attribute in Atom; only the former
        // reaches here (Atom links are handled via the attribute).
        "title" | "link" | "description" | "summary" | "pubdate" | "updated" | "content"
    )
}

/// Parse an RSS or Atom document. Errors only if the XML is unreadable; a
/// valid-but-empty feed yields an empty item list.
pub fn parse_feed(xml: &str) -> Result<Feed> {
    let mut reader = Reader::from_str(xml);
    let mut feed = Feed::default();
    let mut item: Option<FeedItem> = None;
    let mut capture: Option<String> = None;
    let mut buf = String::new();

    loop {
        match reader.read_event().context("malformed feed XML")? {
            Event::Start(e) => {
                let name = local_name_lower(e.name().as_ref());
                if name == "item" || name == "entry" {
                    item = Some(FeedItem::default());
                } else if let Some(link) = atom_link_href(&e) {
                    if let Some(it) = item.as_mut() {
                        it.link = link;
                    }
                } else if item.is_some() {
                    if is_captured(&name) {
                        capture = Some(name);
                        buf.clear();
                    }
                } else if name == "title" {
                    capture = Some(FEED_TITLE.to_string());
                    buf.clear();
                }
            }
            Event::Empty(e) => {
                // Atom links are empty elements: <link href="..."/>
                if let Some(link) = atom_link_href(&e) {
                    if let Some(it) = item.as_mut() {
                        it.link = link;
                    }
                }
            }
            Event::Text(t) => {
                if capture.is_some() {
                    let raw = t.decode().unwrap_or_default();
                    match quick_xml::escape::unescape(&raw) {
                        Ok(text) => buf.push_str(&text),
                        Err(_) => buf.push_str(&raw),
                    }
                }
            }
            Event::CData(t) => {
                if capture.is_some() {
                    // CDATA is already literal text.
                    buf.push_str(&t.decode().unwrap_or_default());
                }
            }
            // Entity references arrive as their own event (e.g. `&amp;`), not
            // inline in the Text event, so resolve them into the buffer.
            Event::GeneralRef(r) => {
                if capture.is_some() {
                    let name = r.decode().unwrap_or_default();
                    if let Some(ch) = resolve_entity(&name) {
                        buf.push(ch);
                    }
                }
            }
            Event::End(e) => {
                let name = local_name_lower(e.name().as_ref());
                if name == "item" || name == "entry" {
                    if let Some(it) = item.take() {
                        feed.items.push(it);
                    }
                } else if let Some(kind) = capture.clone() {
                    if kind == name || (kind == FEED_TITLE && name == "title") {
                        let text = buf.trim().to_string();
                        if kind == FEED_TITLE {
                            if feed.title.is_empty() {
                                feed.title = text;
                            }
                        } else if let Some(it) = item.as_mut() {
                            match kind.as_str() {
                                "title" => it.title = text,
                                "link" => it.link = text,
                                "description" | "summary" => it.summary = text,
                                "pubdate" | "updated" => it.published = text,
                                "content" if it.summary.is_empty() => it.summary = text,
                                _ => {}
                            }
                        }
                        capture = None;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(feed)
}

/// Resolve an XML entity reference name (without `&`/`;`) to a character.
/// Returns `None` for names we do not know (dropped, as browsers do).
fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some('\u{a0}'),
        _ => {}
    }
    let code = name.strip_prefix('#')?;
    let value = if let Some(hex) = code.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        code.parse::<u32>().ok()?
    };
    char::from_u32(value)
}

/// Lowercased local part of a tag name (drops any `ns:` prefix).
fn local_name_lower(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let local = s.rsplit(':').next().unwrap_or(&s);
    local.to_ascii_lowercase()
}

/// `href` of an Atom `<link>` element, if this element is one.
fn atom_link_href(e: &quick_xml::events::BytesStart<'_>) -> Option<String> {
    if local_name_lower(e.name().as_ref()) != "link" {
        return None;
    }
    e.try_get_attribute("href")
        .ok()
        .flatten()
        .and_then(|a| a.unescape_value().ok().map(|v| v.to_string()))
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
  <title>Example Feed</title>
  <item>
    <title>First &amp; Foremost</title>
    <link>https://example.com/1</link>
    <description>Short summary one.</description>
    <pubDate>Mon, 01 Jan 2026 07:00:00 GMT</pubDate>
  </item>
  <item>
    <title>Second</title>
    <link>https://example.com/2</link>
    <description><![CDATA[Summary two]]></description>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Example</title>
  <entry>
    <title>Entry One</title>
    <link href="https://example.org/a"/>
    <summary>Atom summary.</summary>
    <updated>2026-01-02T07:00:00Z</updated>
  </entry>
</feed>"#;

    #[test]
    fn parses_rss_items() {
        let f = parse_feed(RSS).unwrap();
        assert_eq!(f.title, "Example Feed");
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].title, "First & Foremost");
        assert_eq!(f.items[0].link, "https://example.com/1");
        assert_eq!(f.items[0].summary, "Short summary one.");
        assert_eq!(f.items[0].published, "Mon, 01 Jan 2026 07:00:00 GMT");
        assert_eq!(f.items[1].summary, "Summary two");
    }

    #[test]
    fn parses_atom_entries() {
        let f = parse_feed(ATOM).unwrap();
        assert_eq!(f.title, "Atom Example");
        assert_eq!(f.items.len(), 1);
        assert_eq!(f.items[0].title, "Entry One");
        assert_eq!(f.items[0].link, "https://example.org/a");
        assert_eq!(f.items[0].summary, "Atom summary.");
        assert_eq!(f.items[0].published, "2026-01-02T07:00:00Z");
    }

    #[test]
    fn empty_feed_is_ok() {
        let f = parse_feed("<rss><channel><title>Empty</title></channel></rss>").unwrap();
        assert_eq!(f.title, "Empty");
        assert!(f.items.is_empty());
    }

    #[test]
    fn mismatched_tags_error() {
        assert!(parse_feed("<a><b></c></a>").is_err());
    }

    #[test]
    fn non_xml_is_an_empty_feed() {
        // quick-xml is lenient: plain text parses as no elements. That is
        // fine — a feed with no items is reported as such, not a crash.
        let f = parse_feed("not xml at all").unwrap();
        assert!(f.items.is_empty());
    }

    #[test]
    fn entity_refs_resolve() {
        assert_eq!(resolve_entity("amp"), Some('&'));
        assert_eq!(resolve_entity("#65"), Some('A'));
        assert_eq!(resolve_entity("#x41"), Some('A'));
        assert_eq!(resolve_entity("nope"), None);
    }
}

