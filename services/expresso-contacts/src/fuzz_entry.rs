//! Fuzz entry points — compiled only with `--features fuzzing`.

use crate::domain::vcard::parse;

/// Fuzz the vCard parser. Input may be arbitrary bytes; we attempt
/// UTF-8 decoding and bail cleanly if the input is not valid UTF-8.
pub fn fuzz_vcard(data: &[u8]) {
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = parse(s);
    }
}
