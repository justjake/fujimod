//! Read-only display decoding shared by real devices and emulated devices.
pub mod frame;
pub mod packed_color;
pub mod raster;
mod registers;
pub use registers::Registers;

#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);
impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}
impl From<String> for Error {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// A hardware-indexed plane. IDs carry no application meaning.
#[derive(Clone, Debug)]
pub struct Layer {
    pub id: String,
    pub plane: raster::Plane,
}
#[derive(Clone, Debug)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
}
impl Output {
    pub fn render(
        &self,
        mut read: impl FnMut(u64, usize) -> std::result::Result<Vec<u8>, String>,
    ) -> std::result::Result<Vec<u8>, String> {
        let mut rgba = canvas(self.width, self.height)?;
        for l in &self.layers {
            raster::composite(
                |a, n| read(a, n),
                l.plane,
                self.width,
                self.height,
                &mut rgba,
            )?;
        }
        Ok(rgba)
    }
}
pub fn pixel_bytes(width: u32, height: u32) -> std::result::Result<usize, String> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 16_777_216
    {
        return Err("Display canvas exceeds capture bounds".into());
    }
    Ok(width as usize * height as usize * 4)
}
pub fn canvas(width: u32, height: u32) -> std::result::Result<Vec<u8>, String> {
    let mut rgba = vec![0; pixel_bytes(width, height)?];
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Ok(rgba)
}
/// Top-down BITMAPV4 with explicit BGRA masks. Input is straight RGBA.
pub fn bmp(width: u32, height: u32, rgba: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let expected = pixel_bytes(width, height)?;
    if rgba.len() != expected {
        return Err("BMP pixel extent differs".into());
    }
    let mut b = vec![0u8; 122 + expected];
    b[..2].copy_from_slice(b"BM");
    let mut word = |at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    word(2, (122 + expected) as u32);
    word(10, 122);
    word(14, 108);
    word(18, width);
    word(22, (-(height as i32)) as u32);
    word(30, 3);
    word(34, expected as u32);
    word(54, 0x00ff0000);
    word(58, 0x0000ff00);
    word(62, 0x000000ff);
    word(66, 0xff000000);
    word(70, 0x73524742);
    b[26..28].copy_from_slice(&1u16.to_le_bytes());
    b[28..30].copy_from_slice(&32u16.to_le_bytes());
    for (d, s) in b[122..].chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bmp_is_top_down_with_explicit_alpha_and_uncompressed_bgra() {
        let b = bmp(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]).unwrap();
        assert_eq!(&b[..2], b"BM");
        assert_eq!(u32::from_le_bytes(b[10..14].try_into().unwrap()), 122);
        assert_eq!(i32::from_le_bytes(b[22..26].try_into().unwrap()), -1);
        assert_eq!(&b[122..], &[0, 0, 255, 255, 0, 255, 0, 128]);
        assert!(bmp(0, 1, &[]).is_err());
        assert!(bmp(2, 1, &[0; 4]).is_err());
    }
}
