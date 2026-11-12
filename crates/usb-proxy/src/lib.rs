//! Camera-independent USB forwarding. No Fujifilm framing, firmware or policy.
#![forbid(unsafe_code)]
mod client;
pub mod pipeline;
pub mod protocol;
pub use client::{Client, Outcome, RemoteDevice};
pub use protocol::{
    Completion, Device, Endpoint, Interface, Message, Operation, Recipient, RequestType, Setup,
};

#[derive(Debug, thiserror::Error)]
#[error("USB proxy: {0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;

impl From<fujimod_webrtc::Error> for Error {
    fn from(error: fujimod_webrtc::Error) -> Self {
        Self(error.to_string())
    }
}

/// Every USB message fits in one WebRTC message.
const _: () = assert!(protocol::MAX_MESSAGE <= fujimod_webrtc::MAX_MESSAGE);

#[cfg(all(feature = "relay", unix))]
pub mod dev_shares;
#[cfg(all(feature = "relay", not(target_arch = "wasm32")))]
pub mod relay;

#[cfg(target_arch = "wasm32")]
mod browser_peer;
