//! Linear DNG output, written strip by strip.
//!
//! The image is three-sample LinearRaw, 16 bits per sample, uncompressed,
//! one TIFF strip per merge strip. Strip sizes are known in advance, so the
//! header with every strip offset is written first and strips are appended
//! as the merge produces them.

use crate::merge::StripSink;
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub struct DngInfo {
    pub make: String,
    pub model: String,
    /// White-balance multipliers `[R, G, B]`; the neutral is their inverse.
    pub wb: [f32; 3],
    /// XYZ (D65) to camera RGB.
    pub color_matrix: [f64; 9],
}

/// XYZ (D65) to linear sRGB, used when no calibrated camera matrix is known:
/// camera RGB is then treated as having sRGB primaries.
pub const XYZ_TO_SRGB: [f64; 9] = [
    3.2406, -1.5372, -0.4986, -0.9689, 1.8758, 0.0415, 0.0557, -0.2040, 1.0570,
];

enum Value {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Byte(Vec<u8>),
    Ascii(String),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
}

impl Value {
    fn kind_count(&self) -> (u16, u32) {
        match self {
            Value::Byte(v) => (1, v.len() as u32),
            Value::Ascii(s) => (2, s.len() as u32 + 1),
            Value::Short(v) => (3, v.len() as u32),
            Value::Long(v) => (4, v.len() as u32),
            Value::Rational(v) => (5, v.len() as u32),
            Value::SRational(v) => (10, v.len() as u32),
        }
    }

    fn bytes(&self) -> Vec<u8> {
        match self {
            Value::Byte(v) => v.clone(),
            Value::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                b
            }
            Value::Short(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Value::Long(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
            Value::Rational(v) => v
                .iter()
                .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                .collect(),
            Value::SRational(v) => v
                .iter()
                .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                .collect(),
        }
    }
}

pub struct DngWriter {
    out: BufWriter<File>,
    width: usize,
    height: usize,
    strip_rows: usize,
    next_row: usize,
    scratch: Vec<u8>,
}

impl DngWriter {
    pub fn create(
        path: impl AsRef<Path>,
        width: usize,
        height: usize,
        strip_rows: usize,
        info: &DngInfo,
    ) -> io::Result<Self> {
        let strips = height.div_ceil(strip_rows);
        let strip_bytes = |k: usize| (strip_rows.min(height - k * strip_rows) * width * 6) as u32;
        let neutral: Vec<(u32, u32)> = info
            .wb
            .iter()
            .map(|&g| (((1.0 / g as f64) * 1_000_000.0).round() as u32, 1_000_000))
            .collect();
        let mut entries: Vec<(u16, Value)> = vec![
            (254, Value::Long(vec![0])),
            (256, Value::Long(vec![width as u32])),
            (257, Value::Long(vec![height as u32])),
            (258, Value::Short(vec![16, 16, 16])),
            (259, Value::Short(vec![1])),
            (262, Value::Short(vec![34892])),
            (271, Value::Ascii(info.make.clone())),
            (272, Value::Ascii(info.model.clone())),
            (273, Value::Long(vec![0; strips])),
            (274, Value::Short(vec![1])),
            (277, Value::Short(vec![3])),
            (278, Value::Long(vec![strip_rows as u32])),
            (279, Value::Long((0..strips).map(strip_bytes).collect())),
            (284, Value::Short(vec![1])),
            (305, Value::Ascii("fujimod-multishot".into())),
            (50706, Value::Byte(vec![1, 4, 0, 0])),
            (50707, Value::Byte(vec![1, 1, 0, 0])),
            (50708, Value::Ascii(format!("{} {}", info.make, info.model))),
            (50714, Value::Long(vec![0, 0, 0])),
            (50717, Value::Long(vec![65535, 65535, 65535])),
            (
                50721,
                Value::SRational(
                    info.color_matrix
                        .iter()
                        .map(|&v| ((v * 10000.0).round() as i32, 10000))
                        .collect(),
                ),
            ),
            (50728, Value::Rational(neutral)),
            (50778, Value::Short(vec![21])),
        ];
        entries.sort_by_key(|e| e.0);
        let ifd_at = 8u32;
        let ifd_len = 2 + 12 * entries.len() as u32 + 4;
        let mut extra_len = 0u32;
        for (_, v) in &entries {
            let n = v.bytes().len() as u32;
            if n > 4 {
                extra_len += n.next_multiple_of(2);
            }
        }
        let data_at = (ifd_at + ifd_len + extra_len).next_multiple_of(16);
        let offsets: Vec<u32> = (0..strips)
            .scan(data_at, |at, k| {
                let o = *at;
                *at += strip_bytes(k);
                Some(o)
            })
            .collect();
        let total = data_at as u64 + width as u64 * height as u64 * 6;
        if total > u32::MAX as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "image too large for a classic TIFF DNG",
            ));
        }
        for e in entries.iter_mut() {
            if e.0 == 273 {
                e.1 = Value::Long(offsets.clone());
            }
        }
        let mut head = Vec::new();
        head.extend_from_slice(b"II*\0");
        head.extend_from_slice(&ifd_at.to_le_bytes());
        head.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut extra = Vec::new();
        let extra_at = ifd_at + ifd_len;
        for (tag, v) in &entries {
            let (kind, count) = v.kind_count();
            let bytes = v.bytes();
            head.extend_from_slice(&tag.to_le_bytes());
            head.extend_from_slice(&kind.to_le_bytes());
            head.extend_from_slice(&count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut b = bytes.clone();
                b.resize(4, 0);
                head.extend_from_slice(&b);
            } else {
                head.extend_from_slice(&(extra_at + extra.len() as u32).to_le_bytes());
                extra.extend_from_slice(&bytes);
                if extra.len() % 2 == 1 {
                    extra.push(0);
                }
            }
        }
        head.extend_from_slice(&0u32.to_le_bytes());
        head.extend_from_slice(&extra);
        head.resize(data_at as usize, 0);
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&head)?;
        Ok(Self {
            out,
            width,
            height,
            strip_rows,
            next_row: 0,
            scratch: Vec::new(),
        })
    }

    pub fn finish(mut self) -> io::Result<()> {
        if self.next_row != self.height {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "DNG is missing rows",
            ));
        }
        self.out.flush()?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

impl StripSink for DngWriter {
    fn write_strip(&mut self, y0: usize, rows: usize, rgb: &[f32]) -> io::Result<()> {
        if y0 != self.next_row || (rows != self.strip_rows && y0 + rows != self.height) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DNG strips must arrive in order at the declared size",
            ));
        }
        self.scratch.clear();
        self.scratch.reserve(rows * self.width * 6);
        for &v in rgb {
            let q = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
            self.scratch.extend_from_slice(&q.to_le_bytes());
        }
        self.out.write_all(&self.scratch)?;
        self.next_row += rows;
        Ok(())
    }
}
