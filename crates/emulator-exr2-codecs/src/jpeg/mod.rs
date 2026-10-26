//! EXR II JPEG channels 8/9. Firmware owns file metadata; encoder DMA emits
//! a complete baseline JPEG from SOI through EOI.
pub mod decoder;
pub mod encoder;
pub use decoder::Decoder;
pub use encoder::Encoder;
#[cfg(test)]
mod tests;
