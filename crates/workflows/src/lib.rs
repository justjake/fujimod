//! Camera workflows: firmware policy composed from `fujimod-usb` operations.
//!
//! Each module is one workflow. Callers supply qualified firmware facts and
//! compiled payloads; protocol mechanisms live in `fujimod-usb`. Nothing
//! here selects a camera by model name or retries an uncertain operation.
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod access;
mod callback;
pub mod flash;
pub mod bulk;
pub mod debug_mode;
pub mod install;
#[cfg(feature = "service-capture")]
pub mod service_capture;
pub mod service_mode;
pub mod state;

pub use access::Limits;
