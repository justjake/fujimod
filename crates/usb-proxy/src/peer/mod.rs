/// Which local candidates ICE may advertise for a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidatePolicy {
    /// Host, server-reflexive and relayed candidates.
    Any,
    /// Only a TURN relayed candidate. The connection never sends from a local
    /// address to the peer, so it works through any NAT that reaches TURN over UDP.
    RelayOnly,
    /// Host and server-reflexive candidates only. No TURN relay is allocated
    /// and the peer's relayed candidates are dropped, so no byte crosses a
    /// relay.
    DirectOnly,
}

#[cfg(not(target_arch = "wasm32"))]
mod datagram;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::PeerSocket;
#[cfg(not(target_arch = "wasm32"))]
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
pub use host::{HostEvent, HostRoom};
#[cfg(target_arch = "wasm32")]
mod browser;
#[cfg(not(target_arch = "wasm32"))]
mod turn;
#[cfg(target_arch = "wasm32")]
pub(crate) use browser::PeerSocket;
