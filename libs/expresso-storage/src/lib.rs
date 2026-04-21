//! S3/MinIO abstraction layer

pub mod client;
pub mod error;

pub use client::ObjectStore;
