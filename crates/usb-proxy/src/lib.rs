//! Camera-independent USB forwarding. No Fujifilm framing, firmware or policy.
#![forbid(unsafe_code)]
mod client;
pub mod protocol;
pub use client::{Client, Outcome, Progress, RemoteDevice};
pub use protocol::{
    Completion, Device, Endpoint, Interface, Message, Operation, Recipient, RequestType, Setup,
};

#[derive(Debug, thiserror::Error)]
#[error("USB proxy: {0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(all(feature = "relay", unix))]
pub mod dev_shares;
#[cfg(all(feature = "relay", not(target_arch = "wasm32")))]
pub mod relay;

mod peer;
pub mod share;
#[cfg(all(feature = "relay", not(target_arch = "wasm32")))]
mod signaling;
