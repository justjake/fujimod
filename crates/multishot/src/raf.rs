//! Reader for X-Trans RAF files.
//!
//! A RAF starts with a fixed header whose big-endian offset table locates the
//! embedded JPEG (carrying EXIF), a big-endian metadata record list and the
//! raw section. The raw section is a TIFF stream whose Fuji directory (tag
//! 0xf000) gives the stored plane size, sample depth, data offset, data length
//! and black levels. Uncompressed 16-bit-container planes are read in place;
//! lossless compressed planes are decoded by [`crate::raf_lossless`] on first
//! use into a temporary uncompressed copy. Lossy compressed planes are
//! refused.

use crate::raf_lossless;
use crate::source::{Calibration, Exposure, FrameSource};
use crate::tiff::{Tiff, find};
use crate::xtrans::XTrans;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

const MAGIC: &[u8; 16] = b"FUJIFILMCCD-RAW ";

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rect {
    pub left: usize,
    pub top: usize,
    pub width: usize,
    pub height: usize,
}

/// How the raw plane is stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Compression {
    Uncompressed,
    Lossless,
}

/// A decoded plane in a temporary file, removed when dropped.
#[derive(Debug)]
struct DecodedPlane {
    path: PathBuf,
}

impl Drop for DecodedPlane {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

#[derive(Clone, Debug)]
pub struct Raf {
    pub path: PathBuf,
    pub model: String,
    /// Stored plane width and height, including any masked border.
    pub raw_width: usize,
    pub raw_height: usize,
    /// Active image area within the stored plane.
    pub active: Rect,
    /// CFA layout relative to the stored plane's `(0, 0)`.
    pub cfa_raw: XTrans,
    pub bits: u32,
    /// Black levels as recorded (one value, or 36 per tile position).
    pub black_recorded: Vec<u32>,
    /// White balance multipliers recorded as shot, `[G, R, B]` counts.
    pub wb_grb: Option<[u32; 3]>,
    pub compression: Compression,
    /// File offset of the plane, or of the compressed stream.
    pub data_offset: u64,
    /// Bytes of the plane, or of the compressed stream.
    pub data_len: u64,
    pub exposure: Exposure,
    calibration: Calibration,
    cfa_active: XTrans,
    decoded: Arc<OnceLock<Result<DecodedPlane, String>>>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn read_at(file: &mut File, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Metadata records: big-endian count, then `(tag, size, bytes)` records.
fn parse_meta(buf: &[u8]) -> io::Result<Vec<(u16, Vec<u8>)>> {
    if buf.len() < 4 {
        return Err(bad("RAF metadata block is truncated"));
    }
    let count = be32(buf, 0) as usize;
    if count > 1024 {
        return Err(bad(format!("RAF metadata claims {count} records")));
    }
    let mut at = 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if at + 4 > buf.len() {
            return Err(bad("RAF metadata record header is truncated"));
        }
        let tag = be16(buf, at);
        let size = be16(buf, at + 2) as usize;
        at += 4;
        let data = buf
            .get(at..at + size)
            .ok_or_else(|| bad(format!("RAF metadata record {tag:#06x} is truncated")))?;
        out.push((tag, data.to_vec()));
        at += size;
    }
    Ok(out)
}

fn meta_pair(meta: &[(u16, Vec<u8>)], tag: u16) -> Option<(usize, usize)> {
    meta.iter()
        .find(|(t, d)| *t == tag && d.len() >= 4)
        .map(|(_, d)| (be16(d, 0) as usize, be16(d, 2) as usize))
}

fn parse_exif(jpeg: &[u8]) -> Exposure {
    let mut exposure = Exposure::default();
    let Some(start) = jpeg.windows(6).position(|w| w == b"Exif\0\0") else {
        return exposure;
    };
    let Ok(tiff) = Tiff::new(&jpeg[start + 6..]) else {
        return exposure;
    };
    let Ok(ifd0) = tiff.first_ifd().and_then(|at| tiff.ifd(at)) else {
        return exposure;
    };
    let Some(exif_at) = find(&ifd0, 0x8769).and_then(|e| tiff.uint(e).ok()) else {
        return exposure;
    };
    let Ok(exif) = tiff.ifd(exif_at as usize) else {
        return exposure;
    };
    let rational = |tag| find(&exif, tag).and_then(|e| tiff.rational(e).ok());
    let ascii = |tag| find(&exif, tag).and_then(|e| tiff.ascii(e).ok());
    exposure.seconds = rational(0x829a);
    exposure.f_number = rational(0x829d);
    exposure.iso = find(&exif, 0x8827).and_then(|e| tiff.uint(e).ok());
    exposure.bias_ev = rational(0x9204);
    exposure.focal_mm = rational(0x920a);
    exposure.datetime = ascii(0x9003);
    exposure.subsec = ascii(0x9291);
    exposure
}

impl Raf {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let header = read_at(&mut file, 0, 108)?;
        if &header[0..16] != MAGIC {
            return Err(bad("not a RAF file (header magic mismatch)"));
        }
        let model = String::from_utf8_lossy(&header[28..60])
            .trim_end_matches('\0')
            .trim()
            .to_string();
        let jpeg_offset = be32(&header, 84) as u64;
        let jpeg_len = be32(&header, 88) as usize;
        let meta_offset = be32(&header, 92) as u64;
        let meta_len = be32(&header, 96) as usize;
        let raw_offset = be32(&header, 100) as u64;
        let raw_len = be32(&header, 104) as u64;
        for (name, off, len) in [
            ("JPEG", jpeg_offset, jpeg_len as u64),
            ("metadata", meta_offset, meta_len as u64),
            ("raw", raw_offset, raw_len),
        ] {
            if off == 0 || off + len > file_len {
                return Err(bad(format!("RAF {name} section lies outside the file")));
            }
        }

        let meta = parse_meta(&read_at(&mut file, meta_offset, meta_len.min(1 << 20))?)?;
        let layout = meta
            .iter()
            .find(|(t, d)| *t == 0x131 && d.len() == 36)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| {
                bad("RAF has no X-Trans layout record (0x131); only X-Trans RAFs are supported")
            })?;
        // The layout is stored last site first.
        let mut rows = [[0u8; 6]; 6];
        for (i, &c) in layout.iter().enumerate() {
            let k = 35 - i;
            rows[k / 6][k % 6] = c & 3;
        }
        let cfa_raw = XTrans::new(rows).map_err(bad)?;
        let wb_grb = meta
            .iter()
            .find(|(t, d)| *t == 0x2ff0 && d.len() >= 6)
            .map(|(_, d)| [be16(d, 0) as u32, be16(d, 2) as u32, be16(d, 4) as u32]);

        // Raw section TIFF stream: the Fuji directory holds the plane.
        let raw_head = read_at(&mut file, raw_offset, (raw_len as usize).min(1 << 16))?;
        let tiff = Tiff::new(&raw_head).map_err(bad)?;
        let ifd0 = tiff.ifd(tiff.first_ifd().map_err(bad)?).map_err(bad)?;
        let fuji_at = find(&ifd0, 0xf000)
            .ok_or_else(|| bad("RAF raw section has no Fuji directory (0xf000)"))
            .and_then(|e| tiff.uint(e).map_err(bad))? as usize;
        let fuji = tiff.ifd(fuji_at).map_err(bad)?;
        let get = |tag: u16| -> io::Result<u32> {
            find(&fuji, tag)
                .ok_or_else(|| bad(format!("Fuji directory lacks tag {tag:#06x}")))
                .and_then(|e| tiff.uint(e).map_err(bad))
        };
        let raw_width = get(0xf001)? as usize;
        let raw_height = get(0xf002)? as usize;
        let bits = get(0xf003)?;
        let strip_offset = get(0xf007)? as u64;
        let strip_bytes = get(0xf008)? as u64;
        let black_recorded = find(&fuji, 0xf00a)
            .map(|e| tiff.uints(e).map_err(bad))
            .transpose()?
            .unwrap_or_default();
        // As-shot white balance: the metadata record when present, else the
        // Fuji directory's WB_GRBLevels (0xf00e).
        let wb_grb = wb_grb.or_else(|| {
            let v = tiff.uints(find(&fuji, 0xf00e)?).ok()?;
            (v.len() >= 3).then(|| [v[0], v[1], v[2]])
        });

        if !(8..=16).contains(&bits) {
            return Err(bad(format!("unsupported sample depth of {bits} bits")));
        }
        let plane_bytes = raw_width as u64 * raw_height as u64 * 2;
        let data_offset = raw_offset + strip_offset;
        let (compression, data_len) = if strip_bytes < plane_bytes {
            (Compression::Lossless, strip_bytes)
        } else {
            (Compression::Uncompressed, plane_bytes)
        };
        if data_offset + data_len > file_len {
            return Err(bad("RAF raw plane runs past the end of the file"));
        }
        if compression == Compression::Lossless {
            let head = read_at(&mut file, data_offset, 16.min(data_len as usize))?;
            let h = raf_lossless::read_header(&head).map_err(|e| bad(e.to_string()))?;
            if (h.width, h.height, h.bits) != (raw_width, raw_height, bits) {
                return Err(bad(format!(
                    "compressed RAF plane header says {}x{} at {} bits; the Fuji directory says \
                     {raw_width}x{raw_height} at {bits} bits",
                    h.width, h.height, h.bits
                )));
            }
        }

        let (crop_top, crop_left) = meta_pair(&meta, 0x110).unwrap_or((0, 0));
        let (crop_h, crop_w) =
            meta_pair(&meta, 0x111).unwrap_or((raw_height - crop_top, raw_width - crop_left));
        if crop_left + crop_w > raw_width
            || crop_top + crop_h > raw_height
            || crop_w < 6
            || crop_h < 6
        {
            return Err(bad("RAF active area lies outside the raw plane"));
        }
        let active = Rect {
            left: crop_left,
            top: crop_top,
            width: crop_w,
            height: crop_h,
        };
        let cfa_active = cfa_raw.shifted(active.left, active.top);

        let mut black = [[0f32; 6]; 6];
        match black_recorded.len() {
            0 => return Err(bad("RAF records no black level")),
            36 => {
                for y in 0..6 {
                    for x in 0..6 {
                        let (ry, rx) = ((y + active.top) % 6, (x + active.left) % 6);
                        black[y][x] = black_recorded[ry * 6 + rx] as f32;
                    }
                }
            }
            n => {
                let mean = black_recorded.iter().map(|&v| v as f64).sum::<f64>() / n as f64;
                black = [[mean as f32; 6]; 6];
            }
        }
        let white = ((1u32 << bits) - 1) as f32;
        let wb = match wb_grb {
            Some([g, r, b]) if g > 0 && r > 0 && b > 0 => {
                [r as f32 / g as f32, 1.0, b as f32 / g as f32]
            }
            _ => [1.0, 1.0, 1.0],
        };

        let jpeg = read_at(&mut file, jpeg_offset, jpeg_len.min(1 << 18))?;
        let exposure = parse_exif(&jpeg);

        Ok(Self {
            path: path.to_path_buf(),
            model,
            raw_width,
            raw_height,
            active,
            cfa_raw,
            bits,
            black_recorded,
            wb_grb,
            compression,
            data_offset,
            data_len,
            exposure,
            calibration: Calibration { black, white, wb },
            cfa_active,
            decoded: Arc::default(),
        })
    }

    /// File and offset holding the uncompressed plane. A compressed plane is
    /// decoded on first use; clones share the decoded copy.
    fn plane_source(&self) -> io::Result<(&Path, u64)> {
        if self.compression == Compression::Uncompressed {
            return Ok((&self.path, self.data_offset));
        }
        match self
            .decoded
            .get_or_init(|| self.decode().map_err(|e| e.to_string()))
        {
            Ok(plane) => Ok((&plane.path, 0)),
            Err(e) => Err(bad(e.clone())),
        }
    }

    fn decode(&self) -> io::Result<DecodedPlane> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut file = File::open(&self.path)?;
        let stream = read_at(&mut file, self.data_offset, self.data_len as usize)?;
        let mut plane = vec![0u16; self.raw_width * self.raw_height];
        raf_lossless::decode(&stream, &self.cfa_raw.rows(), &mut plane)
            .map_err(|e| bad(format!("{}: {e}", self.path.display())))?;
        drop(stream);
        let decoded = DecodedPlane {
            path: std::env::temp_dir().join(format!(
                "fujimod-multishot-{}-{}.plane",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )),
        };
        let bytes: Vec<u8> = plane.iter().flat_map(|v| v.to_le_bytes()).collect();
        drop(plane);
        std::fs::write(&decoded.path, bytes)?;
        Ok(decoded)
    }
}

impl FrameSource for Raf {
    fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    fn width(&self) -> usize {
        self.active.width
    }
    fn height(&self) -> usize {
        self.active.height
    }
    fn cfa(&self) -> XTrans {
        self.cfa_active
    }
    fn calibration(&self) -> &Calibration {
        &self.calibration
    }
    fn exposure(&self) -> Exposure {
        self.exposure.clone()
    }
    fn read_rows(&self, y0: usize, out: &mut [u16]) -> io::Result<()> {
        let w = self.active.width;
        let rows = out.len() / w;
        if y0 + rows > self.active.height {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "rows past active area",
            ));
        }
        let (path, offset) = self.plane_source()?;
        let mut file = File::open(path)?;
        let stride = self.raw_width * 2;
        let first = offset + ((self.active.top + y0) * stride) as u64;
        file.seek(SeekFrom::Start(first))?;
        let mut buf = vec![0u8; rows * stride];
        file.read_exact(&mut buf)?;
        for r in 0..rows {
            let src = &buf[r * stride + self.active.left * 2..][..w * 2];
            for (o, b) in out[r * w..(r + 1) * w].iter_mut().zip(src.chunks_exact(2)) {
                *o = u16::from_le_bytes([b[0], b[1]]);
            }
        }
        Ok(())
    }
}

/// Writes a minimal uncompressed RAF with the layout `Raf::open` reads. Used
/// by tests and to package synthetic frames.
pub struct RafFixture<'a> {
    pub model: &'a str,
    pub raw_width: usize,
    pub raw_height: usize,
    pub active: Rect,
    pub cfa_raw: XTrans,
    pub bits: u32,
    pub black: u32,
    pub wb_grb: [u16; 3],
    pub exposure_seconds: (u32, u32),
    pub iso: u16,
    /// Stored-plane samples, row-major.
    pub data: &'a [u16],
}

impl RafFixture<'_> {
    pub fn write(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let plane: Vec<u8> = self.data.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.write_with_plane(path, &plane)
    }

    /// Writes the fixture with its plane in lossless compressed form.
    #[cfg(test)]
    fn write_lossless(&self, path: impl AsRef<Path>, strip_width: usize) -> io::Result<()> {
        let stream = raf_lossless::encode(
            self.data,
            self.raw_width,
            self.raw_height,
            strip_width,
            &self.cfa_raw.rows(),
            self.bits,
        );
        self.write_with_plane(path, &stream)
    }

    fn write_with_plane(&self, path: impl AsRef<Path>, plane: &[u8]) -> io::Result<()> {
        // Embedded JPEG: SOI, APP1 EXIF (big-endian TIFF), EOI.
        let mut t = Vec::new();
        t.extend_from_slice(b"MM\0*");
        t.extend_from_slice(&8u32.to_be_bytes());
        // IFD0: one entry pointing at the EXIF directory at 26.
        t.extend_from_slice(&1u16.to_be_bytes());
        t.extend_from_slice(&0x8769u16.to_be_bytes());
        t.extend_from_slice(&4u16.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&26u32.to_be_bytes());
        t.extend_from_slice(&0u32.to_be_bytes());
        // EXIF directory: exposure time (rational at 56) and ISO.
        t.extend_from_slice(&2u16.to_be_bytes());
        t.extend_from_slice(&0x829au16.to_be_bytes());
        t.extend_from_slice(&5u16.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&56u32.to_be_bytes());
        t.extend_from_slice(&0x8827u16.to_be_bytes());
        t.extend_from_slice(&3u16.to_be_bytes());
        t.extend_from_slice(&1u32.to_be_bytes());
        t.extend_from_slice(&self.iso.to_be_bytes());
        t.extend_from_slice(&[0, 0]);
        t.extend_from_slice(&0u32.to_be_bytes());
        t.extend_from_slice(&self.exposure_seconds.0.to_be_bytes());
        t.extend_from_slice(&self.exposure_seconds.1.to_be_bytes());
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(&t);
        jpeg.extend_from_slice(&[0xff, 0xd9]);

        // Metadata records.
        let mut records: Vec<(u16, Vec<u8>)> = Vec::new();
        let pair = |a: usize, b: usize| {
            let mut v = (a as u16).to_be_bytes().to_vec();
            v.extend_from_slice(&(b as u16).to_be_bytes());
            v
        };
        records.push((0x100, pair(self.raw_height, self.raw_width)));
        records.push((0x110, pair(self.active.top, self.active.left)));
        records.push((0x111, pair(self.active.height, self.active.width)));
        let rows = self.cfa_raw.rows();
        let mut layout = vec![0u8; 36];
        for k in 0..36 {
            layout[35 - k] = rows[k / 6][k % 6];
        }
        records.push((0x131, layout));
        records.push((
            0x2ff0,
            self.wb_grb.iter().flat_map(|v| v.to_be_bytes()).collect(),
        ));
        let mut meta = (records.len() as u32).to_be_bytes().to_vec();
        for (tag, data) in &records {
            meta.extend_from_slice(&tag.to_be_bytes());
            meta.extend_from_slice(&(data.len() as u16).to_be_bytes());
            meta.extend_from_slice(data);
        }

        // Raw section: TIFF header, IFD0 -> Fuji directory, black levels, plane.
        let fuji_entries: [(u16, u16, u32, u32); 6] = [
            (0xf001, 4, 1, self.raw_width as u32),
            (0xf002, 4, 1, self.raw_height as u32),
            (0xf003, 4, 1, self.bits),
            (0xf007, 4, 1, 0),
            (0xf008, 4, 1, plane.len() as u32),
            (0xf00a, 4, 36, 0),
        ];
        let fuji_at = 8 + 2 + 12 + 4;
        let black_at = fuji_at + 2 + 12 * fuji_entries.len() + 4;
        let data_at = (black_at + 36 * 4).next_multiple_of(16);
        let mut r = Vec::new();
        r.extend_from_slice(b"II*\0");
        r.extend_from_slice(&8u32.to_le_bytes());
        r.extend_from_slice(&1u16.to_le_bytes());
        r.extend_from_slice(&0xf000u16.to_le_bytes());
        r.extend_from_slice(&13u16.to_le_bytes());
        r.extend_from_slice(&1u32.to_le_bytes());
        r.extend_from_slice(&(fuji_at as u32).to_le_bytes());
        r.extend_from_slice(&0u32.to_le_bytes());
        r.extend_from_slice(&(fuji_entries.len() as u16).to_le_bytes());
        for (tag, kind, count, value) in fuji_entries {
            let value = match tag {
                0xf007 => data_at as u32,
                0xf00a => black_at as u32,
                _ => value,
            };
            r.extend_from_slice(&tag.to_le_bytes());
            r.extend_from_slice(&kind.to_le_bytes());
            r.extend_from_slice(&count.to_le_bytes());
            r.extend_from_slice(&value.to_le_bytes());
        }
        r.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..36 {
            r.extend_from_slice(&self.black.to_le_bytes());
        }
        r.resize(data_at, 0);
        r.extend_from_slice(plane);

        let jpeg_offset = 160u32;
        let meta_offset = jpeg_offset + jpeg.len() as u32;
        let raw_offset = (meta_offset + meta.len() as u32).next_multiple_of(16);
        let mut out = Vec::with_capacity(raw_offset as usize + r.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(b"0201");
        out.extend_from_slice(b"FF000000");
        let mut model = self.model.as_bytes().to_vec();
        model.resize(32, 0);
        out.extend_from_slice(&model);
        out.extend_from_slice(b"0100");
        out.resize(84, 0);
        for v in [
            jpeg_offset,
            jpeg.len() as u32,
            meta_offset,
            meta.len() as u32,
            raw_offset,
            r.len() as u32,
        ] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.resize(jpeg_offset as usize, 0);
        out.extend_from_slice(&jpeg);
        out.extend_from_slice(&meta);
        out.resize(raw_offset as usize, 0);
        out.extend_from_slice(&r);
        std::fs::write(path, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_round_trips_through_the_reader() {
        let (rw, rh) = (48usize, 30usize);
        let data: Vec<u16> = (0..rw * rh).map(|i| (i % 4000) as u16 + 1024).collect();
        let active = Rect {
            left: 4,
            top: 3,
            width: 36,
            height: 24,
        };
        let fixture = RafFixture {
            model: "X-H2S",
            raw_width: rw,
            raw_height: rh,
            active,
            cfa_raw: XTrans::standard(),
            bits: 14,
            black: 1024,
            wb_grb: [302, 580, 610],
            exposure_seconds: (1, 1000),
            iso: 400,
            data: &data,
        };
        let dir = std::env::temp_dir().join(format!("multishot-raf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.RAF");
        fixture.write(&path).unwrap();
        let raf = Raf::open(&path).unwrap();
        assert_eq!(raf.model, "X-H2S");
        assert_eq!((raf.raw_width, raf.raw_height, raf.bits), (rw, rh, 14));
        assert_eq!(raf.active, active);
        assert_eq!(raf.cfa_raw, XTrans::standard());
        assert_eq!(raf.cfa(), XTrans::standard().shifted(4, 3));
        assert_eq!(raf.exposure.seconds, Some(0.001));
        assert_eq!(raf.exposure.iso, Some(400));
        assert_eq!(raf.calibration().black[0][0], 1024.0);
        assert!((raf.calibration().wb[0] - 580.0 / 302.0).abs() < 1e-6);
        let mut rows = vec![0u16; 36 * 2];
        raf.read_rows(5, &mut rows).unwrap();
        assert_eq!(rows[0], data[(3 + 5) * rw + 4]);
        assert_eq!(rows[36 + 35], data[(3 + 6) * rw + 4 + 35]);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn fixture(rw: usize, rh: usize, data: &[u16]) -> RafFixture<'_> {
        RafFixture {
            model: "X-H2S",
            raw_width: rw,
            raw_height: rh,
            active: Rect {
                left: 6,
                top: 6,
                width: rw - 12,
                height: rh - 12,
            },
            cfa_raw: XTrans::standard(),
            bits: 14,
            black: 1024,
            wb_grb: [302, 580, 610],
            exposure_seconds: (1, 500),
            iso: 200,
            data,
        }
    }

    #[test]
    fn lossless_compressed_planes_read_like_uncompressed() {
        let (rw, rh) = (120usize, 42usize);
        let data: Vec<u16> = (0..rw * rh)
            .map(|i| 1024 + ((i % rw) * 37 + (i / rw) * 91 + (i * i) % 113) as u16 % 6000)
            .collect();
        let dir = std::env::temp_dir().join(format!("multishot-lossless-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.RAF");
        let packed = dir.join("packed.RAF");
        fixture(rw, rh, &data).write(&plain).unwrap();
        fixture(rw, rh, &data).write_lossless(&packed, 48).unwrap();
        let a = Raf::open(&plain).unwrap();
        let b = Raf::open(&packed).unwrap();
        assert_eq!(a.compression, Compression::Uncompressed);
        assert_eq!(b.compression, Compression::Lossless);
        assert!(b.data_len < a.data_len);
        let (w, h) = (a.width(), a.height());
        let mut ra = vec![0u16; w * h];
        let mut rb = vec![0u16; w * h];
        a.read_rows(0, &mut ra).unwrap();
        b.read_rows(0, &mut rb).unwrap();
        assert!(ra == rb, "decoded plane differs from the uncompressed one");
        let mut part = vec![0u16; w * 5];
        b.clone().read_rows(7, &mut part).unwrap();
        assert!(part[..] == ra[7 * w..12 * w]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refuses_corrupt_and_lossy_compressed_planes() {
        let (rw, rh) = (48usize, 24usize);
        let data: Vec<u16> = (0..rw * rh).map(|i| 1024 + (i % 977) as u16).collect();
        let dir = std::env::temp_dir().join(format!("multishot-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("packed.RAF");
        fixture(rw, rh, &data).write_lossless(&path, 24).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let raf = Raf::open(&path).unwrap();
        let stream_at = raf.data_offset as usize;

        // A flipped bit inside the first strip's codes.
        let mut bad_bits = bytes.clone();
        bad_bits[stream_at + 64 + 20] ^= 0x10;
        let flipped = dir.join("flipped.RAF");
        std::fs::write(&flipped, &bad_bits).unwrap();
        let raf = Raf::open(&flipped).unwrap();
        let mut rows = vec![0u16; raf.width() * 6];
        let err = raf.read_rows(0, &mut rows).unwrap_err().to_string();
        assert!(err.contains("corrupt compressed RAF plane"), "{err}");

        // Format version 0 is lossy compression.
        let mut lossy = bytes.clone();
        lossy[stream_at + 2] = 0;
        let lossy_path = dir.join("lossy.RAF");
        std::fs::write(&lossy_path, &lossy).unwrap();
        let err = Raf::open(&lossy_path).unwrap_err().to_string();
        assert!(err.contains("lossy compression"), "{err}");

        // Short plane without a compressed-stream header.
        let short = vec![0u16; rw * rh / 2];
        let raw = dir.join("short.RAF");
        fixture(rw, rh, &short).write(&raw).unwrap();
        let err = Raf::open(&raw).unwrap_err().to_string();
        assert!(err.contains("compressed RAF"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
