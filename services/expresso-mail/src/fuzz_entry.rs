//! Fuzz entry points — compiled only with `--features fuzzing`.
//! Internal parsers are exposed here as `pub fn fuzz_*` wrappers so
//! `fuzz/fuzz_targets/` can call them without leaking API surface.

use crate::imip::extract_imip_reply;
use crate::sieve::evaluate;

/// Fuzz the iMIP MIME parser with arbitrary bytes.
pub fn fuzz_imip(data: &[u8]) {
    let _ = extract_imip_reply(data);
}

/// Fuzz the sieve script evaluator with arbitrary bytes for both
/// the script and the raw message.
pub fn fuzz_sieve(data: &[u8]) {
    // Split input at the midpoint: first half = script, second half = message.
    let mid = data.len() / 2;
    let _ = evaluate(&data[..mid], &data[mid..]);
}
