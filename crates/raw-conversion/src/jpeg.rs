//! Reads what a converted JPEG says about itself: its frame size and the
//! scalar tags of its Fujifilm MakerNote (film mode 0x1401, grain 0x1047 and
//! so on), recorded raw so a manifest shows what the camera applied.
use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub width: u16,
    pub height: u16,
    /// MakerNote tag (as `0x1401`) to its value, for one-value integer tags.
    pub maker_note: BTreeMap<String, i64>,
}

pub fn inspect(jpeg: &[u8]) -> Result<Summary, String> {
    if !jpeg.starts_with(&[0xff, 0xd8]) {
        return Err("not a JPEG".into());
    }
    let mut out = Summary::default();
    let mut at = 2;
    while at + 4 <= jpeg.len() {
        if jpeg[at] != 0xff {
            return Err(format!("marker expected at {at}"));
        }
        let marker = jpeg[at + 1];
        let length = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        let body = jpeg
            .get(at + 4..at + 2 + length)
            .ok_or("truncated segment")?;
        match marker {
            0xe1 if body.starts_with(b"Exif\0\0") => out.maker_note = maker_note(&body[6..]),
            // Baseline, extended or progressive frame header.
            0xc0..=0xc2 if body.len() >= 5 => {
                out.height = u16::from_be_bytes([body[1], body[2]]);
                out.width = u16::from_be_bytes([body[3], body[4]]);
                return Ok(out);
            }
            0xda => break,
            _ => {}
        }
        at += 2 + length;
    }
    Err("no frame header before the scan".into())
}

struct Tiff<'a> {
    bytes: &'a [u8],
    little: bool,
}
impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }
    /// (tag, type, count, value-or-offset position) for each IFD entry.
    fn entries(&self, ifd: usize) -> Vec<(u16, u16, u32, usize)> {
        let count = self.u16(ifd).unwrap_or(0) as usize;
        (0..count)
            .filter_map(|i| {
                let e = ifd + 2 + 12 * i;
                Some((self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?, e + 8))
            })
            .collect()
    }
}

fn maker_note(tiff: &[u8]) -> BTreeMap<String, i64> {
    let little = tiff.starts_with(b"II");
    let exif = Tiff {
        bytes: tiff,
        little,
    };
    let find = |t: &Tiff, ifd: usize, tag: u16| t.entries(ifd).into_iter().find(|e| e.0 == tag);
    let Some(ifd0) = exif.u32(4) else {
        return BTreeMap::new();
    };
    let Some((.., exif_at)) = find(&exif, ifd0 as usize, 0x8769) else {
        return BTreeMap::new();
    };
    let Some(exif_ifd) = exif.u32(exif_at) else {
        return BTreeMap::new();
    };
    let Some((_, _, count, note_at)) = find(&exif, exif_ifd as usize, 0x927c) else {
        return BTreeMap::new();
    };
    let Some(start) = exif.u32(note_at) else {
        return BTreeMap::new();
    };
    let Some(note) = tiff.get(start as usize..start as usize + count as usize) else {
        return BTreeMap::new();
    };
    // "FUJIFILM", then the IFD offset; offsets are relative to the note and little-endian.
    if !note.starts_with(b"FUJIFILM") {
        return BTreeMap::new();
    }
    let fuji = Tiff {
        bytes: note,
        little: true,
    };
    let Some(ifd) = fuji.u32(8) else {
        return BTreeMap::new();
    };
    fuji.entries(ifd as usize)
        .into_iter()
        .filter(|e| e.2 == 1)
        .filter_map(|(tag, kind, _, at)| {
            let value = match kind {
                3 => fuji.u16(at)? as i64,
                4 => fuji.u32(at)? as i64,
                8 => fuji.u16(at)? as i16 as i64,
                9 => fuji.u32(at)? as i32 as i64,
                _ => return None,
            };
            Some((format!("{tag:#06x}"), value))
        })
        .collect()
}

/// A minimal JPEG with an Exif Fujifilm MakerNote, for tests.
#[cfg(test)]
pub fn synthetic(width: u16, height: u16, tags: &[(u16, u16)]) -> Vec<u8> {
    let mut note = b"FUJIFILM".to_vec();
    note.extend_from_slice(&12u32.to_le_bytes());
    note.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    for &(tag, value) in tags {
        note.extend_from_slice(&tag.to_le_bytes());
        note.extend_from_slice(&3u16.to_le_bytes());
        note.extend_from_slice(&1u32.to_le_bytes());
        note.extend_from_slice(&(value as u32).to_le_bytes());
    }
    // TIFF header, IFD0 at 8 with one ExifIFD pointer, Exif IFD at 26 with the note at 44.
    let mut tiff = b"II*\0".to_vec();
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&26u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x927cu16.to_le_bytes());
    tiff.extend_from_slice(&7u16.to_le_bytes());
    tiff.extend_from_slice(&(note.len() as u32).to_le_bytes());
    tiff.extend_from_slice(&44u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(tiff.len(), 44);
    tiff.extend_from_slice(&note);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&[0xff, 0xc0, 0, 11, 8]);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&[1, 1, 0x11, 0, 0xff, 0xd9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_frame_size_and_maker_note() {
        let jpeg = synthetic(6240, 4160, &[(0x1401, 0x700), (0x1047, 32)]);
        let s = inspect(&jpeg).unwrap();
        assert_eq!((s.width, s.height), (6240, 4160));
        assert_eq!(s.maker_note["0x1401"], 0x700);
        assert_eq!(s.maker_note["0x1047"], 32);
        assert!(inspect(b"not a jpeg").is_err());
    }
}
