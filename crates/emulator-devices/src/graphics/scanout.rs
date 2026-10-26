//! A captured output image. Publication and archival naming belong to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub output: usize,
    /// Deterministic image-change sequence from the modeled output.
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
