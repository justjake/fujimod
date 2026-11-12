//! WebRTC data channels between peers that meet in a share-link room.
//!
//! A room is named by a [`share::Ticket`]. Its owner joins with a
//! [`HostRoom`] and answers each controller that connects with a
//! [`PeerSocket`]. The signaling service ([`signaling`], feature `signaling`)
//! only relays sealed offers and answers; it never sees their contents.
#![forbid(unsafe_code)]

mod policy;
pub use policy::{CandidatePolicy, Pacing};
pub mod share;

#[cfg(not(target_arch = "wasm32"))]
mod host;
#[cfg(not(target_arch = "wasm32"))]
mod ice;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
mod runtime;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod turn;
#[cfg(not(target_arch = "wasm32"))]
pub use host::{HostEvent, HostRoom};
#[cfg(not(target_arch = "wasm32"))]
pub use native::PeerSocket;

#[cfg(all(feature = "signaling", not(target_arch = "wasm32")))]
pub mod signaling;

#[derive(Debug, thiserror::Error)]
#[error("WebRTC: {0}")]
pub struct Error(pub String);
pub type Result<T> = std::result::Result<T, Error>;

/// Receives one line for each connection step.
pub type Progress = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// The largest message a [`PeerSocket`] carries: an 8 MiB transfer and 64 KiB
/// of framing.
pub const MAX_MESSAGE: usize = 8 * 1024 * 1024 + 65536;
