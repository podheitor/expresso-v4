//! WBXML encoder: [`Event`] stream → bytes.

use crate::{global, Event, CHARSET_UTF8, PUBLIC_ID_UNKNOWN, WBXML_VERSION};

/// Encode a document (header + body) to WBXML bytes. The encoder tracks the
/// active codepage and emits `SWITCH_PAGE` automatically when an element's
/// `page` differs from the current one.
pub fn encode(events: &[Event]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + events.len() * 4);
    // Header (WAP-192 §5.1): version, public id, charset, string-table length.
    out.push(WBXML_VERSION);
    out.push(PUBLIC_ID_UNKNOWN);
    out.push(CHARSET_UTF8);
    out.push(0x00); // empty string table

    let mut page: u8 = 0;
    for ev in events {
        match ev {
            Event::StartElement {
                page: p,
                token,
                has_content,
            } => {
                if *p != page {
                    out.push(global::SWITCH_PAGE);
                    out.push(*p);
                    page = *p;
                }
                let mut t = *token & global::TAG_MASK;
                if *has_content {
                    t |= global::HAS_CONTENT;
                }
                out.push(t);
            }
            Event::EndElement => out.push(global::END),
            Event::Text(s) => {
                out.push(global::STR_I);
                out.extend_from_slice(s.as_bytes());
                out.push(0x00); // NUL-terminated inline string
            }
            Event::Opaque(bytes) => {
                out.push(global::OPAQUE);
                write_mb_u32(&mut out, bytes.len() as u32);
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// Write a WBXML multi-byte unsigned integer (WAP-192 §5.4): base-128,
/// big-endian, continuation bit (0x80) set on all but the last byte.
pub(crate) fn write_mb_u32(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    i -= 1;
    buf[i] = (v & 0x7F) as u8;
    v >>= 7;
    while v != 0 {
        i -= 1;
        buf[i] = (v & 0x7F) as u8 | 0x80;
        v >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mb(v: u32) -> Vec<u8> {
        let mut o = Vec::new();
        write_mb_u32(&mut o, v);
        o
    }

    #[test]
    fn mb_u32_single_byte() {
        assert_eq!(mb(0), vec![0x00]);
        assert_eq!(mb(127), vec![0x7F]);
    }

    #[test]
    fn mb_u32_two_bytes() {
        // 128 = 0x81 0x00
        assert_eq!(mb(128), vec![0x81, 0x00]);
        // 300 = 0b10_0101100 → 0x82 0x2C
        assert_eq!(mb(300), vec![0x82, 0x2C]);
    }

    #[test]
    fn mb_u32_large() {
        // 0x4000 = 16384 → 0x81 0x80 0x00
        assert_eq!(mb(16384), vec![0x81, 0x80, 0x00]);
    }

    #[test]
    fn start_element_with_content_sets_bit() {
        let b = encode(&[Event::start(0, 0x05), Event::EndElement]);
        // After the 4-byte header: tag 0x05 | 0x40 = 0x45, then END 0x01.
        assert_eq!(b[4], 0x45);
        assert_eq!(b[5], 0x01);
    }

    #[test]
    fn empty_element_no_content_bit() {
        let b = encode(&[Event::empty(0, 0x05)]);
        assert_eq!(b[4], 0x05);
    }

    #[test]
    fn switch_page_emitted_once() {
        let b = encode(&[Event::empty(7, 0x12), Event::empty(7, 0x13)]);
        // header(4) then SWITCH_PAGE 0x00, page 0x07, tag 0x12, tag 0x13.
        assert_eq!(&b[4..8], &[0x00, 0x07, 0x12, 0x13]);
    }

    #[test]
    fn text_is_nul_terminated() {
        let b = encode(&[Event::Text("hi".into())]);
        assert_eq!(&b[4..], &[global::STR_I, b'h', b'i', 0x00]);
    }
}
