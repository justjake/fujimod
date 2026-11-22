//! Cacheability of a virtual address from the ARMv7-A short-descriptor
//! translation tables: sections, supersections, large and small pages.
//!
//! Only the TEX, C and B bits are read. Long-descriptor tables and TEX
//! remap are refused instead of guessed.

/// The registers a table walk starts from.
pub(super) struct Walk {
    pub ttbr0: u32,
    pub ttbr1: u32,
    pub ttbcr: u32,
    pub tex_remap: bool,
}

/// Whether memory of these attributes is held in the processor-side caches:
/// strongly ordered and device memory are not, write-through and write-back
/// are, and with TEX[2] set either cache level makes it cacheable.
pub(super) fn cacheable_mapping(tex: u32, c: bool, b: bool) -> bool {
    match tex {
        0 => c,
        t if t & 4 != 0 => c || b || t & 3 != 0,
        _ => false,
    }
}

impl Walk {
    /// `read` returns the physical word at an address.
    pub(super) fn cacheable(
        &self,
        va: u32,
        mut read: impl FnMut(u64) -> Result<u32, String>,
    ) -> Result<bool, String> {
        if self.ttbcr & (1 << 31) != 0 {
            return Err("long-descriptor translation is unsupported by the cache model".into());
        }
        if self.tex_remap {
            return Err("TEX remap is unsupported by the cache model".into());
        }
        let n = self.ttbcr & 7;
        let base = if n != 0 && va >> (32 - n) != 0 {
            self.ttbr1 & !0x3fff
        } else {
            self.ttbr0 & !((1 << (14 - n)) - 1)
        };
        let first = read(u64::from(base) + u64::from(va >> 20) * 4)?;
        let (tex, c, b) = match first & 3 {
            1 => {
                let second = read(u64::from(first & !0x3ff) + u64::from((va >> 12) & 0xff) * 4)?;
                match second & 3 {
                    // A fault aborts the access before it reaches the cache.
                    0 => return Ok(true),
                    1 => ((second >> 12) & 7, second & 8 != 0, second & 4 != 0),
                    _ => ((second >> 6) & 7, second & 8 != 0, second & 4 != 0),
                }
            }
            2 | 3 => ((first >> 12) & 7, first & 8 != 0, first & 4 != 0),
            _ => return Ok(true),
        };
        Ok(cacheable_mapping(tex, c, b))
    }
}
