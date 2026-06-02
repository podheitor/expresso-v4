//! WAP Binary XML (WBXML) codec for Exchange ActiveSync.
//!
//! EAS frames are WBXML (WAP-192) with the MS-ASWBXML token tables. This crate
//! is a focused, dependency-free codec for the subset EAS actually uses:
//! element tags (with/without content), inline strings (`STR_I`), opaque data
//! (`OPAQUE`, used for bodies), and codepage switching (`SWITCH_PAGE`). It does
//! NOT implement attributes or the string table — EAS content does not use them.
//!
//! The model is a flat [`Event`] stream (SAX-like). Build a document as a `Vec`
//! of events and [`encode`] it; parse bytes back with [`decode`]. Tag names live
//! in per-codepage [`tokens`] tables so callers can work with `(page, token)`
//! pairs or resolve human-readable names for debugging.
//!
//! Pure logic over byte slices — no I/O, no allocation beyond the output buffer.

mod decode;
mod encode;
pub mod tokens;

pub use decode::{decode, DecodeError};
pub use encode::encode;

/// WBXML global tokens (WAP-192 §5.8.1) used by EAS.
pub(crate) mod global {
    pub const SWITCH_PAGE: u8 = 0x00;
    pub const END: u8 = 0x01;
    pub const STR_I: u8 = 0x03;
    pub const OPAQUE: u8 = 0xC3;
    /// Bit set on a tag token when the element has content (children/text).
    pub const HAS_CONTENT: u8 = 0x40;
    /// Mask for the tag identity (low 6 bits).
    pub const TAG_MASK: u8 = 0x3F;
}

/// WBXML version byte we emit/accept: `0x03` = WBXML 1.3 (what EAS uses).
pub const WBXML_VERSION: u8 = 0x03;
/// Public identifier `0x01` = "unknown / string-table index 0" — EAS uses this.
pub const PUBLIC_ID_UNKNOWN: u8 = 0x01;
/// Charset `0x6A` = UTF-8 (IANA MIBenum 106), the only charset EAS uses.
pub const CHARSET_UTF8: u8 = 0x6A;

/// A single WBXML event in document order.
///
/// `page` on `StartElement` is the active codepage for `token`; the encoder
/// emits a `SWITCH_PAGE` automatically when the page changes, so callers may
/// set each element's `page` and ignore page bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Open an element. `has_content` must be true iff an `EndElement` will
    /// follow (i.e. the element has children or text). Empty elements set it
    /// false and emit no `EndElement`.
    StartElement {
        page: u8,
        token: u8,
        has_content: bool,
    },
    /// Close the most recently opened content element.
    EndElement,
    /// Inline UTF-8 text content (`STR_I`).
    Text(String),
    /// Opaque binary content (`OPAQUE` + length) — EAS carries message bodies
    /// this way.
    Opaque(Vec<u8>),
}

impl Event {
    /// Convenience: an element with content (children/text follow).
    pub fn start(page: u8, token: u8) -> Self {
        Event::StartElement {
            page,
            token,
            has_content: true,
        }
    }

    /// Convenience: an empty element (no content, no matching `EndElement`).
    pub fn empty(page: u8, token: u8) -> Self {
        Event::StartElement {
            page,
            token,
            has_content: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_simple_element_with_text() {
        // <FolderSync><SyncKey>0</SyncKey></FolderSync> on page 7 (FolderHierarchy).
        let doc = vec![
            Event::start(7, 0x16), // FolderSync
            Event::start(7, 0x12), // SyncKey
            Event::Text("0".into()),
            Event::EndElement,
            Event::EndElement,
        ];
        let bytes = encode(&doc);
        let back = decode(&bytes).unwrap();
        assert_eq!(back, doc);
    }

    #[test]
    fn round_trip_empty_element() {
        let doc = vec![
            Event::start(0, 0x05),
            Event::empty(0, 0x06),
            Event::EndElement,
        ];
        let bytes = encode(&doc);
        assert_eq!(decode(&bytes).unwrap(), doc);
    }

    #[test]
    fn round_trip_opaque_body() {
        let doc = vec![
            Event::start(0, 0x05),
            Event::Opaque(vec![1, 2, 3, 0, 255, 10]),
            Event::EndElement,
        ];
        let bytes = encode(&doc);
        assert_eq!(decode(&bytes).unwrap(), doc);
    }

    #[test]
    fn round_trip_page_switch() {
        // Switch from page 0 (AirSync) to page 7 (FolderHierarchy) mid-document.
        let doc = vec![
            Event::start(0, 0x05),
            Event::start(7, 0x12),
            Event::Text("x".into()),
            Event::EndElement,
            Event::EndElement,
        ];
        let bytes = encode(&doc);
        assert_eq!(decode(&bytes).unwrap(), doc);
    }

    #[test]
    fn header_bytes_are_emitted() {
        let bytes = encode(&[Event::empty(0, 0x05)]);
        assert_eq!(bytes[0], WBXML_VERSION);
        assert_eq!(bytes[1], PUBLIC_ID_UNKNOWN);
        assert_eq!(bytes[2], CHARSET_UTF8);
        assert_eq!(bytes[3], 0x00); // empty string table length
    }
}
