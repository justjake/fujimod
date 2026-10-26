//! Bounded YYCbCr raster layouts and the analysis grid wire.
pub type GridHandle = std::sync::Arc<std::sync::Mutex<[usize; 2]>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageLayout {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}
impl ImageLayout {
    pub fn bytes(self) -> Result<usize, String> {
        let bytes = self.stride.checked_mul(self.height);
        if self.width == 0
            || self.width > 8192
            || self.width & 1 != 0
            || self.height == 0
            || self.height > 8192
            || self.stride < self.width * 2
            || bytes.is_none_or(|v| v > 64 << 20)
        {
            return Err("image raster exceeds its bounded YYCbCr layout".into());
        }
        Ok(bytes.unwrap())
    }
}
