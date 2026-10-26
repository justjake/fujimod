//! EXR II combination, output channels, still processor, scaler, warp and AVC codec.
pub mod avc;
pub mod combination;
pub mod construct;
pub mod output;
pub mod scaler;
mod shading;
pub mod still;
pub mod transfer;
pub mod warp;
pub use avc::AvcCodec;
pub use output::OutputChannel;
pub use scaler::Scaler;
pub use still::StillProcessor;
pub use warp::Warp;

#[cfg(test)]
mod tests;
