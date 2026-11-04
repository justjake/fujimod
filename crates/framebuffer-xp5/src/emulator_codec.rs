//! Emulator-private converter -> LCDC transport. Primary contains unchanged
//! linear native-pixmap bytes (32-bit ARGB / little-endian BGRA); secondary
//! contains this 32-byte header. No CPU-facing input format is changed.
//! Display allocation and consumer behavior.
const MAGIC: &[u8; 8] = b"X6OSD\0\x01\0";
pub const HEADER_BYTES: usize = 32;
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes: usize,
}
impl Header {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 || width > 4095 || height > 4095 {
            return Err("OSD codec dimensions outside 1..4095".into());
        }
        let stride = width * 4;
        Ok(Self {
            width,
            height,
            stride,
            bytes: stride as usize * height as usize,
        })
    }
}
pub fn encode(
    primary: u64,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<[u8; HEADER_BYTES], String> {
    let h = Header::new(width, height)?;
    if pixels.len() != h.bytes {
        return Err("OSD codec input extent mismatch".into());
    }
    let mut out = [0; HEADER_BYTES];
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&(width as u16).to_le_bytes());
    out[10..12].copy_from_slice(&(height as u16).to_le_bytes());
    out[12..16].copy_from_slice(&h.stride.to_le_bytes());
    out[16..20].copy_from_slice(&(h.bytes as u32).to_le_bytes());
    out[20..28].copy_from_slice(&primary.to_le_bytes());
    Ok(out)
}
/// One converter publication. A register pair that selects a different
/// primary has no image from this publication; it is not a corrupt header.
#[derive(Clone, Copy, Debug)]
pub struct Publication {
    pub primary: u64,
    pub geometry: Header,
}
pub fn decode(bytes: &[u8]) -> Result<Publication, String> {
    if bytes.len() != HEADER_BYTES || &bytes[..8] != MAGIC {
        return Err("OSD secondary plane has no private codec header".into());
    }
    let h = Header::new(
        u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as u32,
        u16::from_le_bytes(bytes[10..12].try_into().unwrap()) as u32,
    )?;
    if u32::from_le_bytes(bytes[12..16].try_into().unwrap()) != h.stride
        || u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize != h.bytes
        || bytes[28..32] != [0; 4]
    {
        return Err("OSD private codec header inconsistent with plane".into());
    }
    let primary = u64::from_le_bytes(bytes[20..28].try_into().unwrap());
    if primary == 0
        || primary & 3 != 0
        || primary
            .checked_add(h.bytes as u64)
            .is_none_or(|end| end > 1 << 35)
    {
        return Err("OSD publication primary address exceeds packed-pixel DMA bounds".into());
    }
    Ok(Publication {
        primary,
        geometry: h,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn allocations_and_corrupt_headers() {
        // Largest panel conversion includes the stock extra row; largest
        // doubled HD geometry also fits its reserved primary/secondary pair.
        for (w, h, capacity, secondary) in [
            (1280, 961, 0x4b2000, 0x1000),
            (3840, 2161, 0x21c4000, 0x2000),
        ] {
            let pixels = vec![0x5a; Header::new(w, h).unwrap().bytes];
            let encoded = encode(0x1_89f0000, w, h, &pixels).unwrap();
            let publication = decode(&encoded).unwrap();
            assert_eq!(publication.primary, 0x1_89f0000);
            let decoded = publication.geometry;
            assert!(decoded.bytes <= capacity);
            assert!(encoded.len() <= secondary);
            assert_eq!(decoded.stride, w * 4);
            for n in (0..20).chain(28..32) {
                let mut bad = encoded;
                bad[n] ^= 0x80;
                assert!(decode(&bad).is_err());
            }
        }
        let mut header = encode(0x1_89f0000, 2, 2, &[0; 16]).unwrap();
        for primary in [0u64, 1, (1 << 35) - 4] {
            header[20..28].copy_from_slice(&primary.to_le_bytes());
            assert!(decode(&header).is_err());
        }
        assert!(encode(0, 16, 16, &[0; 4]).is_err());
        assert!(Header::new(0, 600).is_err());
        assert!(Header::new(4096, 1).is_err());
    }
}
