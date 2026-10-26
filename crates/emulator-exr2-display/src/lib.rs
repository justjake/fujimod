//! Two display pipes with clocked scanout, output formatting and compositing.
//! Register masks and routing define the modeled interface. Pixel layouts,
//! phase cadence, reset values and completion latency include declared
//! assumptions. Pipes share a wrapper for two-bank serialized state and event
//! ordering.
pub mod construct;
pub mod device;
pub mod formatter;
#[cfg(test)]
mod pixel_tests;
mod rectangles;
pub mod scanout;
pub mod timing;
pub use device::Scanout;
pub use formatter::Formatter;
pub use timing::Timing;
