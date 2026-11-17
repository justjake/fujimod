//! Minimal TIFF directory reading for RAF and EXIF metadata.

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub tag: u16,
    pub kind: u16,
    pub count: u32,
    /// Offset of the entry's 4-byte value field within the buffer.
    pub value_at: usize,
}

pub struct Tiff<'a> {
    pub buf: &'a [u8],
    pub little: bool,
}

fn type_size(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

impl<'a> Tiff<'a> {
    /// A TIFF stream starting at `buf[0]` with its byte-order header.
    pub fn new(buf: &'a [u8]) -> Result<Self, String> {
        let little = match buf.get(0..4) {
            Some(b"II*\0") => true,
            Some(b"MM\0*") => false,
            _ => return Err("missing TIFF header".into()),
        };
        Ok(Self { buf, little })
    }

    pub fn u16(&self, at: usize) -> Result<u16, String> {
        let b = self.buf.get(at..at + 2).ok_or("TIFF read out of bounds")?;
        Ok(if self.little {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    pub fn u32(&self, at: usize) -> Result<u32, String> {
        let b = self.buf.get(at..at + 4).ok_or("TIFF read out of bounds")?;
        let a = [b[0], b[1], b[2], b[3]];
        Ok(if self.little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    pub fn first_ifd(&self) -> Result<usize, String> {
        Ok(self.u32(4)? as usize)
    }

    pub fn ifd(&self, at: usize) -> Result<Vec<Entry>, String> {
        let count = self.u16(at)? as usize;
        if count > 4096 {
            return Err(format!("TIFF directory at {at} claims {count} entries"));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let e = at + 2 + 12 * i;
            entries.push(Entry {
                tag: self.u16(e)?,
                kind: self.u16(e + 2)?,
                count: self.u32(e + 4)?,
                value_at: e + 8,
            });
        }
        Ok(entries)
    }

    fn data_at(&self, e: &Entry) -> Result<usize, String> {
        let bytes = type_size(e.kind) * e.count as usize;
        if bytes <= 4 {
            Ok(e.value_at)
        } else {
            Ok(self.u32(e.value_at)? as usize)
        }
    }

    /// Unsigned integer values of a BYTE, SHORT, LONG or IFD entry.
    pub fn uints(&self, e: &Entry) -> Result<Vec<u32>, String> {
        let at = self.data_at(e)?;
        let n = e.count as usize;
        if n > 1 << 20 {
            return Err("TIFF entry too large".into());
        }
        (0..n)
            .map(|i| match e.kind {
                1 | 7 => self
                    .buf
                    .get(at + i)
                    .map(|&b| b as u32)
                    .ok_or_else(|| "TIFF read out of bounds".to_string()),
                3 | 8 => self.u16(at + 2 * i).map(u32::from),
                4 | 9 | 13 => self.u32(at + 4 * i),
                k => Err(format!("TIFF tag {:#06x} has non-integer type {k}", e.tag)),
            })
            .collect()
    }

    pub fn uint(&self, e: &Entry) -> Result<u32, String> {
        self.uints(e)?
            .first()
            .copied()
            .ok_or_else(|| format!("TIFF tag {:#06x} is empty", e.tag))
    }

    pub fn rational(&self, e: &Entry) -> Result<f64, String> {
        let at = self.data_at(e)?;
        let (n, d) = (self.u32(at)?, self.u32(at + 4)?);
        let (n, d) = if e.kind == 10 {
            (n as i32 as f64, d as i32 as f64)
        } else {
            (n as f64, d as f64)
        };
        if d == 0.0 {
            return Err(format!("TIFF tag {:#06x} has a zero denominator", e.tag));
        }
        Ok(n / d)
    }

    pub fn ascii(&self, e: &Entry) -> Result<String, String> {
        let at = self.data_at(e)?;
        let bytes = self
            .buf
            .get(at..at + e.count as usize)
            .ok_or("TIFF read out of bounds")?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).trim().to_string())
    }
}

pub fn find(entries: &[Entry], tag: u16) -> Option<&Entry> {
    entries.iter().find(|e| e.tag == tag)
}
