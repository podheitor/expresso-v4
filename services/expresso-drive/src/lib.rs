//! Library surface of expresso-drive.
//!
//! The service runs as a binary (`src/main.rs`); this lib exists only to expose
//! the pure, untrusted-input parsers to the cargo-fuzz harness — a bin-only
//! crate can't be a fuzz dependency. We re-path the leaf modules here rather
//! than restructuring `main.rs`, since `text_extract` is dependency-free and
//! self-contained.

/// Best-effort text extraction for full-text indexing (text/OOXML/PDF).
#[path = "domain/text_extract.rs"]
pub mod text_extract;

#[cfg(feature = "fuzzing")]
pub mod fuzz_entry;
