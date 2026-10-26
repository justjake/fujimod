//! Relocate the stock setup's register-bank tables and reciprocal accessors.
use crate::image::{self, Edit, Result, extent, put, require, word};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copy {
    pub source_offset: usize,
    pub destination_offset: usize,
    pub size: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Banks {
    pub copies: Vec<Copy>,
    pub edits: Vec<Edit>,
}

fn overlaps(a: usize, size: usize, b: usize, end: usize) -> bool {
    a < end && b < a.saturating_add(size)
}

impl Banks {
    pub(super) fn end(&self, stock_length: usize, workspace: usize) -> Result<usize> {
        let mut end = stock_length;
        for copy in &self.copies {
            let limit = copy.destination_offset.checked_add(copy.size);
            require(
                copy.size > 0
                    && copy.destination_offset >= stock_length
                    && limit.is_some_and(|n| n <= workspace),
                "XP5 bank accessor copy escapes free SRAM",
            )?;
            end = end.max(limit.unwrap());
        }
        Ok(end)
    }

    pub(super) fn apply(
        &self,
        prefix: &[u8],
        raw: &mut [u8],
        stock_length: usize,
        stack: usize,
        program_start: usize,
        program_end: usize,
    ) -> Result<()> {
        for (index, copy) in self.copies.iter().enumerate() {
            let start = copy.destination_offset;
            require(
                !overlaps(start, copy.size, 0, stock_length)
                    && !overlaps(
                        start,
                        copy.size,
                        stack.saturating_sub(0x1000),
                        stack + 0x100,
                    )
                    && !overlaps(start, copy.size, program_start, program_end)
                    && self.copies[..index].iter().all(|other| {
                        !overlaps(
                            start,
                            copy.size,
                            other.destination_offset,
                            other.destination_offset.saturating_add(other.size),
                        )
                    }),
                "XP5 bank accessor copy overlaps live SRAM",
            )?;
            let source = extent(prefix, copy.source_offset, copy.size)?;
            image::hash(source, &copy.sha256)?;
            extent(raw, start, copy.size)?;
            raw[start..start + copy.size].copy_from_slice(source);
        }
        let mut addresses = std::collections::BTreeSet::new();
        // Check all expected words before changing any of them.
        for edit in &self.edits {
            require(
                edit.address.is_multiple_of(4)
                    && addresses.insert(edit.address)
                    && (edit
                        .address
                        .checked_add(4)
                        .is_some_and(|n| n <= stock_length)
                        || self.copies.iter().any(|copy| {
                            copy.destination_offset <= edit.address
                                && edit
                                    .address
                                    .checked_add(4)
                                    .is_some_and(|n| n <= copy.destination_offset + copy.size)
                        }))
                    && word(raw, edit.address)? == edit.old,
                "XP5 bank accessor edit differs from source",
            )?;
        }
        for edit in &self.edits {
            put(raw, edit.address, edit.new)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Banks, Vec<u8>, Vec<u8>) {
        let prefix = vec![0x11; 0x4000];
        let banks = Banks {
            copies: vec![Copy {
                source_offset: 0x2000,
                destination_offset: 0x3000,
                size: 16,
                sha256: fujimod_firmware::sha256(&prefix[0x2000..0x2010]),
            }],
            edits: vec![Edit {
                address: 0x3000,
                old: 0x11111111,
                new: 0x22222222,
                what: "Relocate accessor".into(),
            }],
        };
        (banks, prefix, vec![0xff; 0x4000])
    }

    #[test]
    fn copies_are_hashed_and_edits_cannot_modify_stack_loader_or_unloaded_padding() {
        let (banks, prefix, mut raw) = fixture();
        assert_eq!(banks.end(0x1000, 0x38800).unwrap(), 0x3010);
        banks
            .apply(&prefix, &mut raw, 0x1000, 0x2400, 0x3400, 0x3800)
            .unwrap();
        assert_eq!(word(&raw, 0x3000).unwrap(), 0x22222222);
        for case in 0..7 {
            let (mut changed, mut prefix, mut raw) = fixture();
            match case {
                0 => prefix[0x2000] ^= 1,
                1 => changed.copies[0].destination_offset = 0x800,
                2 => changed.copies[0].destination_offset = 0x2000,
                3 => changed.copies[0].destination_offset = 0x3500,
                4 => changed.edits[0].address = 0x3100,
                5 => changed.edits[0].old ^= 1,
                6 => changed.edits.push(changed.edits[0].clone()),
                _ => unreachable!(),
            }
            assert!(
                changed
                    .apply(&prefix, &mut raw, 0x1000, 0x2400, 0x3400, 0x3800)
                    .is_err(),
                "{case}"
            );
        }
    }
}
