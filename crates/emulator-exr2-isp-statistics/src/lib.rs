//! EXR II statistics producer, metering, phase detection and window analysis.
pub mod construct;
pub mod metering;
pub mod phase_detection;
mod pixel_statistics;
pub mod producer;
pub mod window_analysis;
pub use phase_detection::PhaseDetection;
pub use producer::Block18000;
pub use window_analysis::WindowAnalysis;

#[cfg(test)]
mod tests;
