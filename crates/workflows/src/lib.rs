//! Camera workflows: firmware policy composed from `fujimod-usb` operations.
//!
//! Each module is one workflow. Callers supply qualified firmware facts and
//! compiled payloads; protocol mechanisms live in `fujimod-usb`. Nothing
//! here selects a camera by model name or retries an uncertain operation.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod access;
pub mod bulk;
mod callback;
pub mod debug_mode;
pub mod flash;
pub mod install;
pub mod restart;
#[cfg(feature = "service-capture")]
pub mod service_capture;
pub mod service_mode;
pub mod state;
pub mod xt30_201_bulk_words;
pub mod xt30_201_snapshot;

pub use access::Limits;
