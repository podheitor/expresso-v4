//! Best-effort text extraction for full-text indexing.
//!
//! Turns an uploaded file's bytes into plain text for the search index.
//!
//! Handles text/* and a few text-ish application types (json/csv/xml/markdown),
//! decoded as UTF-8 (lossy), and OOXML office docs (docx/xlsx/pptx) — ZIP
//! containers whose relevant XML parts are read and their tag content
//! concatenated. Anything else (PDF, images, binaries) returns `None`.
//!
//! Output is capped at `MAX_TEXT_BYTES` so a huge document can't bloat the
//! index payload. Extraction is intentionally lenient: a malformed archive or
//! non-UTF-8 blob yields `None`/partial text rather than an error — indexing is
//! best-effort and must never fail an upload.

use std::io::Read;

/// Cap on extracted text shipped to the index (256 KiB of characters).
const MAX_TEXT_BYTES: usize = 256 * 1024;

/// Extract indexable text from `bytes` given the file's `mime` and `name`
/// (the name's extension is the fallback when the MIME type is absent/generic).
/// Returns `None` when the format isn't text-extractable.
pub fn extract(mime: Option<&str>, name: &str, bytes: &[u8]) -> Option<String> {
    let kind = classify(mime, name);
    let text = match kind {
        ExtractKind::PlainText => decode_utf8_lossy(bytes),
        ExtractKind::Ooxml => extract_ooxml(bytes)?,
        ExtractKind::Unsupported => return None,
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(cap(trimmed))
}

enum ExtractKind {
    PlainText,
    Ooxml,
    Unsupported,
}

/// Decide how to extract, preferring the MIME type and falling back to the
/// filename extension (browsers often send `application/octet-stream`).
fn classify(mime: Option<&str>, name: &str) -> ExtractKind {
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    if mime.starts_with("text/") || is_text_application(&mime) {
        return ExtractKind::PlainText;
    }
    if is_ooxml_mime(&mime) {
        return ExtractKind::Ooxml;
    }
    match extension(name).as_deref() {
        Some(
            "txt" | "md" | "markdown" | "csv" | "tsv" | "json" | "xml" | "log" | "yaml" | "yml"
            | "toml" | "rs" | "py" | "js" | "ts" | "html" | "css" | "sh",
        ) => ExtractKind::PlainText,
        Some("docx" | "xlsx" | "pptx") => ExtractKind::Ooxml,
        _ => ExtractKind::Unsupported,
    }
}

/// Text-ish `application/*` MIME types we treat as plain text.
fn is_text_application(mime: &str) -> bool {
    matches!(
        mime,
        "application/json"
            | "application/xml"
            | "application/x-yaml"
            | "application/yaml"
            | "application/toml"
            | "application/csv"
            | "application/javascript"
    )
}

fn is_ooxml_mime(mime: &str) -> bool {
    mime.starts_with("application/vnd.openxmlformats-officedocument")
}

fn extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|e| !e.is_empty())
}

fn decode_utf8_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Read an OOXML (ZIP) document and concatenate the text inside its content
/// XML parts. Returns `None` if the archive can't be opened. Per-part read
/// errors are skipped so a partly-corrupt doc still yields what it can.
fn extract_ooxml(bytes: &[u8]) -> Option<String> {
    let reader = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(reader).ok()?;
    let mut out = String::new();
    for i in 0..zip.len() {
        let Ok(mut entry) = zip.by_index(i) else {
            continue;
        };
        if !is_content_part(entry.name()) {
            continue;
        }
        let mut xml = String::new();
        if entry.read_to_string(&mut xml).is_err() {
            continue;
        }
        strip_xml_tags(&xml, &mut out);
        if out.len() >= MAX_TEXT_BYTES {
            break;
        }
    }
    (!out.trim().is_empty()).then_some(out)
}

/// The OOXML parts that carry user-visible text (word body, slide text, shared
/// strings + sheet cells). Other parts (styles, rels, theme) are skipped.
fn is_content_part(name: &str) -> bool {
    name == "word/document.xml"
        || name == "xl/sharedStrings.xml"
        || (name.starts_with("xl/worksheets/") && name.ends_with(".xml"))
        || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
}

/// Append the text between XML tags of `xml` to `out`, inserting a space at each
/// tag boundary so adjacent runs don't fuse. Entities aren't decoded — search
/// tokenization tolerates the few that survive.
fn strip_xml_tags(xml: &str, out: &mut String) {
    let mut in_tag = false;
    let mut last_was_space = out.ends_with(' ') || out.is_empty();
    for c in xml.chars() {
        match c {
            '<' => {
                in_tag = true;
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            _ => {
                out.push(c);
                last_was_space = c.is_whitespace();
            }
        }
    }
}

/// Truncate to `MAX_TEXT_BYTES`, respecting a char boundary.
fn cap(s: &str) -> String {
    if s.len() <= MAX_TEXT_BYTES {
        return s.to_owned();
    }
    let mut end = MAX_TEXT_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_text_by_mime() {
        let t = extract(Some("text/plain"), "a.txt", b"hello world").unwrap();
        assert_eq!(t, "hello world");
    }

    #[test]
    fn extracts_json_application_type() {
        let t = extract(Some("application/json"), "a.json", br#"{"k":"v"}"#).unwrap();
        assert!(t.contains("\"k\":\"v\""));
    }

    #[test]
    fn falls_back_to_extension_when_mime_generic() {
        let t = extract(Some("application/octet-stream"), "notes.md", b"# Title").unwrap();
        assert_eq!(t, "# Title");
    }

    #[test]
    fn unsupported_binary_returns_none() {
        assert!(extract(Some("image/png"), "x.png", &[0u8, 1, 2, 3]).is_none());
    }

    #[test]
    fn pdf_is_not_extracted() {
        // PDF is intentionally out of scope for this slice.
        assert!(extract(Some("application/pdf"), "doc.pdf", b"%PDF-1.7").is_none());
    }

    #[test]
    fn empty_text_returns_none() {
        assert!(extract(Some("text/plain"), "a.txt", b"   \n  ").is_none());
    }

    #[test]
    fn corrupt_ooxml_returns_none() {
        // Not a real zip → archive open fails → None, never panics.
        let bytes = b"PK\x03\x04 not really a zip";
        assert!(extract(
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            "x.docx",
            bytes
        )
        .is_none());
    }

    #[test]
    fn strip_xml_tags_keeps_text_only() {
        let mut out = String::new();
        strip_xml_tags("<w:p><w:t>Hello</w:t> <w:t>World</w:t></w:p>", &mut out);
        assert!(out.contains("Hello"));
        assert!(out.contains("World"));
        assert!(!out.contains('<'));
    }

    #[test]
    fn cap_truncates_at_char_boundary() {
        let big = "a".repeat(MAX_TEXT_BYTES + 100);
        let capped = cap(&big);
        assert!(capped.len() <= MAX_TEXT_BYTES);
    }

    #[test]
    fn extracts_real_docx_zip() {
        // Build a minimal valid docx (zip with word/document.xml) in-memory.
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            use std::io::Write as _;
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(b"<w:document><w:body><w:p><w:t>Indexed body text</w:t></w:p></w:body></w:document>")
                .unwrap();
            zw.finish().unwrap();
        }
        let t = extract(
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            "x.docx",
            &buf,
        )
        .unwrap();
        assert!(t.contains("Indexed body text"), "got: {t}");
    }
}
