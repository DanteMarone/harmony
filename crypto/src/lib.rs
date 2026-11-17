//! Harmony's end-to-end encryption, built on MLS (RFC 9420) through OpenMLS.
//!
//! This crate is deliberately thin: it only wires OpenMLS up for Harmony and
//! contains no cryptography of its own. It compiles to WebAssembly for the
//! browser and to native code for tests, so both run the same logic.
//!
//! One [`Device`] is one browser or app install. Each chat is one MLS group
//! whose group id is the chat id. The server only ever sees the opaque bytes
//! this crate produces.

mod device;
#[cfg(target_arch = "wasm32")]
mod wasm;

pub use device::{Commit, Device, Error, Processed};
