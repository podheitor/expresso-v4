//! Fuzz entry points — compiled only with `--features fuzzing`.
//! Wraps untrusted-input parsers so `fuzz/fuzz_targets/` can drive them.

use crate::text_extract;

/// Fuzz the text-extraction dispatcher with arbitrary bytes, exercising the
/// PDF, OOXML (ZIP), and plain-text paths. Must never panic for any input.
pub fn fuzz_text_extract(data: &[u8]) {
    // Drive each branch of `classify` so one corpus reaches all parsers.
    let _ = text_extract::extract(Some("application/pdf"), "x.pdf", data);
    let _ = text_extract::extract(
        Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "x.docx",
        data,
    );
    let _ = text_extract::extract(Some("text/plain"), "x.txt", data);
}
