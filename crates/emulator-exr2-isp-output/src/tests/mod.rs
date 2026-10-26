#[cfg(not(target_arch = "wasm32"))]
mod avc;
mod avc_decode;
mod checkpoint;
mod dma;
mod fingerprints;
mod jpeg_stream;
mod output;
mod parameters;
mod scaler;
mod still;
mod transfer;
mod warp;
