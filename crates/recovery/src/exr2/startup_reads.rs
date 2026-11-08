//! Flash-loader service reads the card serves from its own image.
//!
//! At startup the stock flash-loader service copies resources such as the
//! message bank and font from internal flash to fixed RAM destinations, and
//! reloads sections the card image already holds. The card places the update's
//! resource bytes at their destinations and replaces each read call, and each
//! reload of a preloaded section, with a successful status.
use crate::{
    Proof,
    image::{self, Plan, Result, Source, extent, put, require, word},
    rom_sd::call,
};
use serde::{Deserialize, Serialize};

/// `mov r0, #0`: the read call's success status.
const SUCCESS: u32 = 0xe3a00000;
const BLOCK: usize = 0x20000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupRead {
    pub what: String,
    pub call: usize,
    pub target: usize,
    pub row: usize,
    pub flash_offset: u32,
    pub destination: usize,
    pub source: Source,
}

/// A section loader that reloads one section-table row the card preloads.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SectionReload {
    pub what: String,
    pub call: usize,
    pub target: usize,
    pub section: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupReads {
    pub check: Proof,
    pub reads: Vec<StartupRead>,
    pub reloads: Vec<SectionReload>,
}

impl StartupReads {
    pub(crate) fn install(&self, inverted: &[u8], plan: &Plan, ram: &mut [u8]) -> Result<()> {
        image::hash(extent(ram, self.check.address, self.check.size)?, &self.check.sha256)?;
        let checked = |at: usize, target: usize| -> Result<()> {
            require(
                at >= self.check.address
                    && at + 4 <= self.check.address + self.check.size
                    && call(ram, at)? == target,
                "Startup read call differs",
            )
        };
        let table = plan
            .regions
            .iter()
            .find(|r| r.name == "section table")
            .ok_or_else(|| image::Error::Invalid("Missing EXR II section table".into()))?;
        for reload in &self.reloads {
            checked(reload.call, reload.target)?;
            let row = table.address + 0x40 + reload.section as usize * 36;
            let (address, size) = (word(ram, row + 4)? as usize, word(ram, row + 8)? as usize);
            require(
                reload.section < 32
                    && word(ram, reload.target + 12)? == 0xe3a00000 | reload.section
                    && word(ram, row)? == reload.section
                    && plan
                        .regions
                        .iter()
                        .any(|r| r.address == address && r.source.decoded_size == size),
                "Section reload is not of a preloaded section",
            )?;
        }
        let mut sources = Vec::new();
        for read in &self.reads {
            checked(read.call, read.target)?;
            let bytes = read.source.read(inverted)?;
            let end = read.destination + bytes.len();
            let row = [word(ram, read.row)?, word(ram, read.row + 4)?, word(ram, read.row + 8)?];
            let blocks = (bytes.len() / BLOCK) as u32;
            require(
                !bytes.is_empty()
                    && bytes.len().is_multiple_of(BLOCK)
                    && row == [read.flash_offset, blocks, read.destination as u32],
                "Startup read table row differs",
            )?;
            let clear = |at: usize, size: usize| end <= at || at + size <= read.destination;
            require(
                plan.regions.iter().all(|r| clear(r.address, r.source.decoded_size))
                    && plan.bss.iter().all(|b| clear(b.address, b.size))
                    && sources.iter().all(|(at, data): &(usize, Vec<u8>)| clear(*at, data.len()))
                    && extent(ram, read.destination, bytes.len())?.iter().all(|b| *b == 0),
                "Startup read destination is occupied",
            )?;
            sources.push((read.destination, bytes));
        }
        for (read, (at, bytes)) in self.reads.iter().zip(&sources) {
            ram[*at..*at + bytes.len()].copy_from_slice(bytes);
            put(ram, read.call, SUCCESS)?;
        }
        for reload in &self.reloads {
            put(ram, reload.call, SUCCESS)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Compression, Region};

    fn fixture() -> (StartupReads, Plan, Vec<u8>, Vec<u8>) {
        let inverted: Vec<u8> = (0..2 * BLOCK).map(|i| (i / 7) as u8 | 1).collect();
        let mut ram = vec![0; 0x100000];
        put(&mut ram, 0x1010, 0xeb0003fa).unwrap();
        put(&mut ram, 0x1020, 0xeb0003fa).unwrap();
        put(&mut ram, 0x1030, 0xeb0003fa).unwrap();
        put(&mut ram, 0x202c, 0xe3a00002).unwrap();
        for (row, values) in [
            (0x3000, [0x40000, 1, 0x80000]),
            (0x3010, [0x60000, 1, 0xa0000]),
            (0x4088, [2, 0xd0000, 0x100]),
        ] {
            for (i, value) in values.into_iter().enumerate() {
                put(&mut ram, row + 4 * i, value).unwrap();
            }
        }
        let source = |at: usize| Source {
            dat_offset: at,
            size: BLOCK,
            compression: Compression::Raw,
            decoded_size: BLOCK,
            sha256: fujimod_firmware::sha256(&inverted[at..at + BLOCK]),
            initialized_prefix: None,
        };
        let read = |what: &str, call, target, row, flash_offset, destination, at| StartupRead {
            what: what.into(),
            call,
            target,
            row,
            flash_offset,
            destination,
            source: source(at),
        };
        let reads = StartupReads {
            check: Proof {
                address: 0x1000,
                size: 0x40,
                sha256: fujimod_firmware::sha256(&ram[0x1000..0x1040]),
            },
            reads: vec![
                read("bank", 0x1010, 0x2000, 0x3000, 0x40000, 0x80000, 0),
                read("font", 0x1020, 0x2010, 0x3010, 0x60000, 0xa0000, BLOCK),
            ],
            reloads: vec![SectionReload {
                what: "section 2".into(),
                call: 0x1030,
                target: 0x2020,
                section: 2,
            }],
        };
        let region = |name: &str, address, decoded_size| Region {
            name: name.into(),
            address,
            source: Source { decoded_size, ..source(0) },
        };
        let plan = Plan {
            size: ram.len(),
            entry: 0,
            regions: vec![
                region("section table", 0x4000, 0x400),
                region("module2", 0xd0000, 0x100),
            ],
            bss: vec![],
            edits: vec![],
        };
        (reads, plan, inverted, ram)
    }

    #[test]
    fn places_each_source_at_its_destination_and_succeeds_the_call() {
        let (reads, plan, inverted, ram) = fixture();
        let mut result = ram.clone();
        reads.install(&inverted, &plan, &mut result).unwrap();
        assert_eq!(&result[0x80000..0x80000 + BLOCK], &inverted[..BLOCK]);
        assert_eq!(&result[0xa0000..0xa0000 + BLOCK], &inverted[BLOCK..]);
        assert_eq!(word(&result, 0x1010).unwrap(), SUCCESS);
        assert_eq!(word(&result, 0x1020).unwrap(), SUCCESS);
        assert_eq!(word(&result, 0x1030).unwrap(), SUCCESS);
        result[0x80000..0x80000 + BLOCK].fill(0);
        result[0xa0000..0xa0000 + BLOCK].fill(0);
        for at in [0x1010, 0x1020, 0x1030] {
            result[at..at + 4].copy_from_slice(&ram[at..at + 4]);
        }
        assert_eq!(result, ram);
    }

    #[test]
    fn changed_call_row_section_or_occupied_destination_refuses_atomically() {
        let (reads, plan, inverted, ram) = fixture();
        for at in [0x1020, 0x3014, 0xa0000, 0x202c, 0x4088, 0x408c] {
            let mut changed = ram.clone();
            changed[at] ^= 1;
            let before = changed.clone();
            assert!(reads.install(&inverted, &plan, &mut changed).is_err());
            assert_eq!(changed, before);
        }
    }
}
