#![doc = include_str!("../README.md")]
//! Asynchronous Fujifilm service/debug USB access.
//!
//! Commands perform one firmware request. Higher-level operations compose them.
//! Firmware-specific limits are supplied explicitly; no camera body is a default.
#![forbid(unsafe_code)]

pub mod connection;
pub mod error;
pub mod ff80;
pub mod operations;
pub mod owner;
pub mod owner_log;
mod proxy;
pub mod remote;
pub mod trace;

pub use error::{Error, Result};

pub mod testing;
