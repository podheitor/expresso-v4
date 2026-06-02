//! WBXML decoder: bytes → [`Event`] stream.

use crate::{global, Event};

/// Errors a malformed WBXML frame can produce. EAS frames come from untrusted
/// clients, so every read is bounds-checked and surfaces a typed error rather
/// than panicking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Ran off the end of the input mid-token.
    UnexpectedEof,
    /// A `SWITCH_PAGE` was not followed by a page byte.
    TruncatedSwitchPage,
    /// An inline string (`STR_I`) had no terminating NUL.
    UnterminatedString,
    /// An inline string was not valid UTF-8.
    InvalidUtf8,
    /// A multi-byte integer ran past the input or exceeded 32 bits.
    BadMultiByte,
    /// An `OPAQUE` length pointed past the end of the input.
    OpaqueOverrun,
    /// A token byte we do not support (attributes, EXT_*, string-table refs).
    UnsupportedToken(u8),
}

/// Decode WBXML bytes (header + body) into an [`Event`] stream. The 4+ byte
/// header (version, public id, charset, string-table) is validated for length
/// and skipped; its values are not interpreted (EAS always uses the same ones).
pub fn decode(bytes: &[u8]) -> Result<Vec<Event>, DecodeError> {
    let mut p = Cursor::new(bytes);
    // Header: version, public id, charset (mb_u32 in spec; EAS uses 1 byte 0x6A).
    p.u8()?; // version
    read_mb_u32(&mut p)?; // public id (mb)
    read_mb_u32(&mut p)?; // charset (mb)
    let strtbl_len = read_mb_u32(&mut p)?; // string table length
    p.skip(strtbl_len as usize)?;

    let mut events = Vec::new();
    let mut page: u8 = 0;
    while let Some(b) = p.peek() {
        match b {
            global::SWITCH_PAGE => {
                p.u8()?;
                page = p.u8().map_err(|_| DecodeError::TruncatedSwitchPage)?;
            }
            global::END => {
                p.u8()?;
                events.push(Event::EndElement);
            }
            global::STR_I => {
                p.u8()?;
                events.push(Event::Text(p.cstring()?));
            }
            global::OPAQUE => {
                p.u8()?;
                let len = read_mb_u32(&mut p)? as usize;
                events.push(Event::Opaque(p.take(len)?.to_vec()));
            }
            // Attributes / extensions / literals / string-table refs are not
            // used by EAS content; reject them explicitly rather than mis-parse.
            0x02 | 0x04 | 0x40 | 0x43 | 0x44 | 0x80 | 0x81 | 0x83 | 0x84 => {
                return Err(DecodeError::UnsupportedToken(b));
            }
            _ => {
                let tok = p.u8()?;
                let has_content = tok & global::HAS_CONTENT != 0;
                events.push(Event::StartElement {
                    page,
                    token: tok & global::TAG_MASK,
                    has_content,
                });
            }
        }
    }
    Ok(events)
}

/// Read a WBXML multi-byte unsigned integer (base-128, high-bit continuation).
fn read_mb_u32(c: &mut Cursor<'_>) -> Result<u32, DecodeError> {
    let mut v: u32 = 0;
    for _ in 0..5 {
        let b = c.u8().map_err(|_| DecodeError::BadMultiByte)?;
        v = v.checked_shl(7).ok_or(DecodeError::BadMultiByte)? | u32::from(b & 0x7F);
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(DecodeError::BadMultiByte)
}

/// A bounds-checked forward cursor over the input.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        let b = *self.buf.get(self.pos).ok_or(DecodeError::UnexpectedEof)?;
        self.pos += 1;
        Ok(b)
    }

    fn skip(&mut self, n: usize) -> Result<(), DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        self.pos = end;
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::OpaqueOverrun)?;
        if end > self.buf.len() {
            return Err(DecodeError::OpaqueOverrun);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Read a NUL-terminated inline UTF-8 string (NUL consumed, not included).
    fn cstring(&mut self) -> Result<String, DecodeError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b == 0 {
                let s = std::str::from_utf8(&self.buf[start..self.pos])
                    .map_err(|_| DecodeError::InvalidUtf8)?
                    .to_owned();
                self.pos += 1; // consume NUL
                return Ok(s);
            }
            self.pos += 1;
        }
        Err(DecodeError::UnterminatedString)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::encode;

    #[test]
    fn decode_rejects_truncated_header() {
        assert_eq!(decode(&[0x03, 0x01]), Err(DecodeError::BadMultiByte));
    }

    #[test]
    fn decode_rejects_unterminated_string() {
        // header + STR_I + bytes with no NUL.
        let bytes = [0x03, 0x01, 0x6A, 0x00, global::STR_I, b'h', b'i'];
        assert_eq!(decode(&bytes), Err(DecodeError::UnterminatedString));
    }

    #[test]
    fn decode_rejects_opaque_overrun() {
        // OPAQUE claims 10 bytes but only 2 follow.
        let bytes = [0x03, 0x01, 0x6A, 0x00, global::OPAQUE, 0x0A, 0x01, 0x02];
        assert_eq!(decode(&bytes), Err(DecodeError::OpaqueOverrun));
    }

    #[test]
    fn decode_rejects_unsupported_token() {
        // 0x80 (EXT_I_0) is not used by EAS content.
        let bytes = [0x03, 0x01, 0x6A, 0x00, 0x80];
        assert_eq!(decode(&bytes), Err(DecodeError::UnsupportedToken(0x80)));
    }

    #[test]
    fn decode_rejects_invalid_utf8() {
        let bytes = [0x03, 0x01, 0x6A, 0x00, global::STR_I, 0xFF, 0xFE, 0x00];
        assert_eq!(decode(&bytes), Err(DecodeError::InvalidUtf8));
    }

    #[test]
    fn empty_body_decodes_to_no_events() {
        let bytes = encode(&[]);
        assert_eq!(decode(&bytes).unwrap(), vec![]);
    }

    #[test]
    fn nested_round_trip() {
        let doc = vec![
            Event::start(0, 0x05),
            Event::start(0, 0x07),
            Event::start(0, 0x08),
            Event::Text("deep".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ];
        assert_eq!(decode(&encode(&doc)).unwrap(), doc);
    }

    #[test]
    fn multibyte_length_opaque_round_trip() {
        // Body of 200 bytes forces a 2-byte mb_u32 length.
        let big = vec![0xABu8; 200];
        let doc = vec![Event::Opaque(big.clone())];
        let back = decode(&encode(&doc)).unwrap();
        assert_eq!(back, vec![Event::Opaque(big)]);
    }
}
