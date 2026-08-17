use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use std::path::PathBuf;

use epub::doc::{EpubDoc, NavPoint};

/// A single chapter extracted from an EPUB, with HTML stripped to plain text.
#[derive(Debug, Clone)]
pub struct Chapter {
    pub title: String,
    pub text: String,
}

/// A parsed EPUB book in memory.
#[derive(Debug, Clone)]
pub struct Book {
    pub title: String,
    pub creator: Option<String>,
    pub chapters: Vec<Chapter>,
}

impl Book {
    pub fn chapter_count(&self) -> usize {
        self.chapters.len()
    }
}

/// Parse an EPUB from raw bytes (a blob already read into memory).
pub fn parse_epub_bytes(bytes: Vec<u8>) -> anyhow::Result<Book> {
    let doc = EpubDoc::from_reader(Cursor::new(bytes))
        .map_err(|e| anyhow::anyhow!("failed to open EPUB: {e}"))?;
    parse_doc(doc)
}

/// Parse an EPUB from a filesystem path.
pub fn parse_epub_path(path: &str) -> anyhow::Result<Book> {
    let doc = EpubDoc::new(path)
        .map_err(|e| anyhow::anyhow!("failed to open EPUB '{path}': {e}"))?;
    parse_doc(doc)
}

fn parse_doc<R: Read + Seek>(mut doc: EpubDoc<R>) -> anyhow::Result<Book> {
    let title = doc.get_title().unwrap_or_else(|| "Untitled".to_string());
    let creator = doc.mdata("creator").map(|m| m.value.clone());
    let toc_titles = build_titles(&doc);

    // Clone spine idrefs up front to avoid a mutable/immutable borrow conflict.
    let spine_ids: Vec<String> = doc.spine.iter().map(|s| s.idref.clone()).collect();

    let mut chapters = Vec::with_capacity(spine_ids.len());
    for (idx, idref) in spine_ids.iter().enumerate() {
        let text = match doc.get_resource_str(idref) {
            Some((html, mime)) if is_html(&html, &mime) => html_to_text(&html),
            _ => String::new(),
        };
        let title = toc_titles
            .get(&idx)
            .cloned()
            .unwrap_or_else(|| fallback_title(idx, &text));
        chapters.push(Chapter { title, text });
    }

    Ok(Book {
        title,
        creator,
        chapters,
    })
}

fn is_html(content: &str, mime: &str) -> bool {
    mime.contains("html")
        || mime.contains("xml")
        || content.trim_start().starts_with('<')
}

/// Map TOC navpoints to spine chapter indices so we can use real chapter
/// titles instead of "Chapter N".
fn build_titles<R: Read + Seek>(doc: &EpubDoc<R>) -> HashMap<usize, String> {
    let mut map = HashMap::new();
    for np in &doc.toc {
        collect_titles(np, doc, &mut map);
    }
    map
}

fn collect_titles<R: Read + Seek>(
    np: &NavPoint,
    doc: &EpubDoc<R>,
    map: &mut HashMap<usize, String>,
) {
    // TOC entries may point to a fragment within a chapter ("chapter.xhtml#sec-1");
    // strip the fragment before mapping to a spine index.
    let content = np.content.to_string_lossy();
    let path = PathBuf::from(content.split('#').next().unwrap_or(""));
    if let Some(idx) = doc.resource_uri_to_chapter(&path) {
        map.entry(idx).or_insert_with(|| np.label.clone());
    }
    for child in &np.children {
        collect_titles(child, doc, map);
    }
}

/// Derive a chapter title when the TOC has no entry: use the first line of
/// text if it looks like a heading, otherwise "Chapter N".
fn fallback_title(idx: usize, text: &str) -> String {
    let first = text.lines().map(|l| l.trim()).find(|l| !l.is_empty());
    match first {
        Some(heading) if !heading.is_empty() && heading.chars().count() <= 100 => {
            heading.to_string()
        }
        _ => format!("Chapter {}", idx + 1),
    }
}

/// Convert chapter HTML to plain text, collapsing blank runs.
fn html_to_text(html: &str) -> String {
    let text = html2text::config::plain()
        .string_from_read(html.as_bytes(), 120)
        .unwrap_or_default();
    normalize_text(&text)
}

/// Trim every line and collapse runs of blank lines into a single blank line
/// (paragraph separator).
fn normalize_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_blank = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !out.is_empty() {
                pending_blank = true;
            }
            continue;
        }
        if pending_blank {
            if !out.ends_with("\n\n") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            pending_blank = false;
        } else if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(trimmed);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_blank_runs() {
        let input = "\n  Hello world  \n\n\n  \nSecond paragraph.\n\n\n";
        let out = normalize_text(input);
        assert_eq!(out, "Hello world\n\nSecond paragraph.");
    }

    #[test]
    fn normalize_strips_outer_whitespace() {
        let out = normalize_text("   \n\n  text  \n  \n");
        assert_eq!(out, "text");
    }

    #[test]
    fn html_to_text_strips_tags_and_decodes_entities() {
        let html = "<html><body><h1>Title</h1><p>Hello &amp; goodbye</p><br><p>Next</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Hello & goodbye"), "got: {text}");
        assert!(text.contains("Next"), "got: {text}");
    }

    #[test]
    fn fallback_title_uses_heading() {
        assert_eq!(fallback_title(0, "The Long Way Home\n\nBody text"), "The Long Way Home");
    }

    #[test]
    fn fallback_title_counts_chapters() {
        assert_eq!(fallback_title(3, ""), "Chapter 4");
    }

    #[test]
    fn fallback_title_rejects_oversized_heading() {
        let long = "x".repeat(200);
        assert_eq!(fallback_title(1, &long), "Chapter 2");
    }

    #[test]
    fn is_html_detects_mimes() {
        assert!(is_html("<html>", "application/xhtml+xml"));
        assert!(is_html("", "text/html"));
        assert!(!is_html("", "image/png"));
        assert!(is_html("<p>hi</p>", "application/octet-stream"));
    }
}
