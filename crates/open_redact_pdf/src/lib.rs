//! Public Rust API for Open Redact PDF.
//!
//! This crate is the stable facade over the internal parser, graphics, text,
//! target normalization, redaction, and writer crates.

mod api;

pub use api::*;
