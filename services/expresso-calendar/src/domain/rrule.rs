//! RRULE expansion for free/busy + occurrence listing.
//!
//! The implementation now lives in the shared `expresso-rrule` crate so that
//! both calendar and meet expand recurrence identically (RFC 5545 §3.3.10).
//! This module re-exports its public API — `Rrule`, `Freq`, `ByDay`, and
//! `single_instance` — so existing call sites keep their paths
//! (`crate::domain::rrule::Rrule`, `super::rrule::single_instance`, …).
//!
//! See `libs/expresso-rrule/src/lib.rs` for the supported subset and tests.

pub use expresso_rrule::{single_instance, Rrule};
