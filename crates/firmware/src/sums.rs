//! Type-6 update integrity words.
//!
//! Each stored word is the 32-bit complement of a byte sum over one region of
//! the update. The regions come from the type-6 inner header, so no camera
//! model is built in. Offsets below are DAT offsets. "Decoded" sums add each
//! byte XOR 0xff; "raw" sums add the bytes as stored. Verified against X100V
//! 3.01 `FPUPDATE.DAT`: every stored word matches and a reseal of the
//! unmodified file is byte-identical. A sum matching does not authenticate a
//! file or show that a camera accepts it.

use crate::{Result, extent, header_value, word};

/// DAT offset of the type-6 inner header (after the 532-byte outer header).
const INNER: usize = 0x214;
/// DAT offset that the inner header's region offsets count from.
const IMAGE: usize = 0x274;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SumBytes {
    /// Sum of each byte XOR 0xff.
    Decoded,
    /// Sum of the bytes as stored.
    Raw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateSum {
    pub name: &'static str,
    /// DAT offset of the stored word.
    pub stored_at: usize,
    /// DAT offset and size of the summed region.
    pub start: usize,
    pub size: usize,
    pub bytes: SumBytes,
    pub stored: u32,
    pub computed: u32,
}

impl UpdateSum {
    pub fn matches(&self) -> bool {
        self.stored == self.computed
    }
}

fn inner(source: &[u8], index: usize) -> Result<usize> {
    Ok(word(source, INNER + 4 * index)? as usize)
}

/// Read every type-6 integrity word and recompute it from the update bytes.
pub fn update_sums(source: &[u8]) -> Result<Vec<UpdateSum>> {
    header_value(0, "update type", word(source, 0)?.into(), 6)?;
    // Inner word 14 ends the flash image and starts the table segment; word 13
    // carries the same value in X100V 3.01.
    let table = inner(source, 14)?;
    let regions = [
        ("image", 0x20c, 0, table, SumBytes::Decoded),
        ("table", 0x210, table, inner(source, 15)?, SumBytes::Decoded),
        (
            "trailer",
            INNER + 8,
            inner(source, 3)?,
            inner(source, 4)?,
            SumBytes::Decoded,
        ),
        (
            "segment-a",
            INNER + 80,
            inner(source, 18)?,
            inner(source, 19)?,
            SumBytes::Raw,
        ),
        (
            "segment-b",
            INNER + 92,
            inner(source, 21)?,
            inner(source, 22)?,
            SumBytes::Raw,
        ),
    ];
    regions
        .into_iter()
        .map(|(name, stored_at, offset, size, bytes)| {
            let start = IMAGE + offset;
            let data = extent(source, start as u64, size as u64)?;
            let sum = data.iter().fold(0u32, |sum, &byte| {
                sum.wrapping_add(u32::from(match bytes {
                    SumBytes::Decoded => byte ^ 0xff,
                    SumBytes::Raw => byte,
                }))
            });
            Ok(UpdateSum {
                name,
                stored_at,
                start,
                size,
                bytes,
                stored: word(source, stored_at)?,
                computed: !sum,
            })
        })
        .collect()
}

/// Rewrite every type-6 integrity word from the current update bytes. No
/// stored word lies inside a summed region, so one pass is final.
pub fn reseal_update_sums(source: &mut [u8]) -> Result<Vec<UpdateSum>> {
    let sums = update_sums(source)?;
    for sum in &sums {
        source[sum.stored_at..sum.stored_at + 4].copy_from_slice(&sum.computed.to_le_bytes());
    }
    Ok(sums)
}
