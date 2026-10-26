//! Checked card-backed setup inputs, with stock DMA retained for PHY writes.
use super::{CodeProof, branch, template};
use crate::image::{self, Result, Source, extent, put, require, word};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    PhyImem,
    PhyDmem,
    LayoutMarker,
    FlashInfo,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub proof: CodeProof,
    pub target: Target,
    pub call: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub source: Source,
    pub file_offset: usize,
    pub destinations: [u64; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardBoot {
    pub hooks: Vec<Hook>,
    pub dma: CodeProof,
    pub layout_marker: u32,
    pub jedec_id: u32,
    pub secondary_sgi: [u64; 3],
    /// Default IMEM/DMEM, then alternate IMEM/DMEM.
    pub payloads: [Payload; 4],
    pub namespace: Option<Source>,
    pub unit_cache: Option<UnitCache>,
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub ram_edits: Vec<image::Edit>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub read: CodeProof,
    pub training_backup: CodeProof,
    pub initialize: CodeProof,
    pub lock: CodeProof,
    pub unlock: CodeProof,
    pub lock_id: u32,
    pub state: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnitCache {
    pub storage_bytes: usize,
    pub source_offset: usize,
    pub destination_offset: usize,
    pub size: usize,
}

pub(super) struct Installation<'a> {
    pub base: u64,
    pub stock: &'a [u8],
    pub program_address: u64,
    pub raw: &'a mut [u8],
    pub program: &'a mut [u8],
    pub ram: &'a mut Vec<u8>,
}

fn verify_code(stock: &[u8], base: u64, proof: &CodeProof) -> Result<usize> {
    let offset = (proof.address as u64)
        .checked_sub(base)
        .ok_or_else(|| image::Error::Invalid("Card setup proof precedes setup".into()))?
        as usize;
    require(
        proof.size >= 4 && offset.is_multiple_of(4),
        "Invalid card setup proof",
    )?;
    image::hash(extent(stock, offset, proof.size)?, &proof.sha256)?;
    Ok(offset)
}

impl CardBoot {
    pub(super) fn resident_end(&self) -> Result<usize> {
        self.unit_cache.as_ref().map_or(Ok(0), |cache| {
            cache
                .destination_offset
                .checked_add(cache.size)
                .ok_or_else(|| image::Error::Invalid("Unit cache extent overflows".into()))
        })
    }

    pub(super) fn copy_unit_cache(
        &self,
        storage: Option<&[u8]>,
        raw: &mut [u8],
        stock_length: usize,
        stack: usize,
        program: std::ops::Range<usize>,
        banks: &super::Banks,
    ) -> Result<()> {
        let Some(cache) = &self.unit_cache else {
            return require(storage.is_none(), "Unexpected storage image");
        };
        let storage = storage.ok_or_else(|| {
            image::Error::Invalid("Card startup requires a unit storage image".into())
        })?;
        let start = cache.destination_offset;
        let end = self.resident_end()?;
        require(
            storage.len() == cache.storage_bytes
                && cache.size > 0
                && start >= stock_length
                && end <= raw.len()
                && (end <= stack.saturating_sub(0x1000) || start >= stack + 0x100)
                && (end <= program.start || start >= program.end)
                && banks.copies.iter().all(|b| {
                    end <= b.destination_offset
                        || start >= b.destination_offset.saturating_add(b.size)
                }),
            "Unit cache overlaps live SRAM or storage size differs",
        )?;
        raw[start..end].copy_from_slice(extent(storage, cache.source_offset, cache.size)?);
        Ok(())
    }

    pub(super) fn install(
        &self,
        inverted: &[u8],
        storage: Option<&[u8]>,
        image: Installation<'_>,
    ) -> Result<()> {
        let Installation {
            base,
            stock,
            program_address,
            raw,
            program,
            ram,
        } = image;
        verify_code(stock, base, &self.dma)?;
        require(
            ram.len() >= 16
                && [0, 0x5555aaac].contains(&self.layout_marker)
                && self.jedec_id > 0
                && self.jedec_id <= 0xffffff
                && self.secondary_sgi.iter().enumerate().all(|(n, &address)| {
                    (0xf0000000..0xffff0000).contains(&address)
                        && address.is_multiple_of(0x10000)
                        && !self.secondary_sgi[..n].contains(&address)
                }),
            "Invalid card setup hardware binding",
        )?;
        let at = template::SETUP_SD_CARD_BOOT;
        let mut empty = vec![0; 216];
        empty[0] = 1;
        require(
            extent(program, at, 216)? == empty,
            "Card setup template changed",
        )?;
        let mut namespace = self
            .namespace
            .as_ref()
            .map(|source| source.read(inverted))
            .transpose()?;
        if let Some(storage) = storage {
            let update = namespace.as_ref().ok_or_else(|| {
                image::Error::Invalid("Storage image lacks a checked update namespace".into())
            })?;
            // An update can end in a partially installed erase page.
            let installed = update.len() & !0xfff;
            require(
                installed > 0 && extent(storage, 0, installed)? == &update[..installed],
                "Unit storage firmware differs from the supplied update",
            )?;
            namespace = Some(storage.to_vec());
        }
        let namespace_offset = (ram.len() + 3) & !3;
        let namespace_size = namespace.as_ref().map_or(0, Vec::len);
        let file_size = if namespace.is_some() {
            namespace_offset
                .checked_add(namespace_size)
                .ok_or_else(|| image::Error::Invalid("Card namespace size overflows".into()))?
        } else {
            ram.len()
        };
        require(file_size < 1 << 30, "Card OS image exceeds loader bounds")?;
        let mut values = vec![
            1,
            1,
            u64::from(self.layout_marker),
            u64::from(self.jedec_id),
            self.dma.address as u64,
            file_size as u64,
            if namespace.is_some() {
                namespace_offset as u64
            } else {
                0
            },
            namespace_size as u64,
        ];
        values.extend(self.secondary_sgi);
        for payload in &self.payloads {
            let bytes = payload.source.read(inverted)?;
            require(
                !bytes.is_empty()
                    && bytes.len() <= 0x10000
                    && bytes.len().is_multiple_of(2)
                    && extent(ram, payload.file_offset, bytes.len())? == bytes
                    && payload
                        .destinations
                        .iter()
                        .all(|&d| (0xf0000000..0xff000000).contains(&d) && d.is_multiple_of(2)),
                "PHY payload differs from the checked card image",
            )?;
            values.extend([
                payload.file_offset as u64,
                bytes.len() as u64,
                payload.destinations[0],
                payload.destinations[1],
            ]);
        }
        require(values.len() == 27, "Card setup configuration size differs")?;
        for (index, value) in values.into_iter().enumerate() {
            program[at + index * 8..at + index * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        let mut targets = std::collections::BTreeSet::new();
        let mut addresses = std::collections::BTreeSet::new();
        for hook in &self.hooks {
            let offset = verify_code(stock, base, &hook.proof)?;
            require(
                targets.insert(hook.target.clone()) && addresses.insert(offset),
                "Duplicate card setup hook",
            )?;
            if hook.call {
                require(
                    word(raw, offset)? & 0xfc000000 == 0x94000000,
                    "Card setup call changed",
                )?;
            } else {
                require(
                    word(raw, offset)? == word(stock, offset)?,
                    "Card setup entry changed",
                )?;
            }
            let symbol = match hook.target {
                Target::PhyImem => template::SETUP_SD_PHY_IMEM,
                Target::PhyDmem => template::SETUP_SD_PHY_DMEM,
                Target::LayoutMarker => template::SETUP_SD_LAYOUT_MARKER,
                Target::FlashInfo => template::SETUP_SD_FLASH_INFO,
            };
            put(
                raw,
                offset,
                branch(
                    hook.proof.address as u64,
                    program_address + symbol as u64,
                    hook.call,
                )?,
            )?;
        }
        require(targets.len() == 4, "Incomplete card setup hooks")?;
        if let Some(runtime) = &self.runtime {
            require(
                namespace_size > 0,
                "Runtime reader lacks its storage namespace",
            )?;
            let proofs = [
                &runtime.read,
                &runtime.training_backup,
                &runtime.initialize,
                &runtime.lock,
                &runtime.unlock,
            ];
            for (index, proof) in proofs.into_iter().enumerate() {
                verify_code(ram, 0, proof)?;
                require(
                    proofs[..index].iter().all(|other| {
                        proof.address >= other.address + other.size
                            || other.address >= proof.address + proof.size
                    }),
                    "Runtime code proofs overlap",
                )?;
            }
            let at = template::SETUP_SD_RUNTIME;
            require(
                extent(program, at, 40)? == [1u64, 0, 0, 0, 0].map(u64::to_le_bytes).concat(),
                "Card runtime template changed",
            )?;
            require(
                runtime.state.is_multiple_of(4)
                    && extent(ram, runtime.state, 28)?.iter().all(|v| *v == 0),
                "Card driver state is not empty RAM",
            )?;
            for (index, value) in [
                1,
                runtime.lock.address as u64,
                runtime.unlock.address as u64,
                u64::from(runtime.lock_id),
                runtime.state as u64,
            ]
            .into_iter()
            .enumerate()
            {
                program[at + index * 8..at + index * 8 + 8].copy_from_slice(&value.to_le_bytes());
            }
            for (proof, symbol) in [
                (&runtime.read, template::SETUP_SD_FIRMWARE_READ),
                (&runtime.training_backup, template::SETUP_SD_TRAINING_BACKUP),
                (&runtime.initialize, template::SETUP_SD_FIRMWARE_INITIALIZE),
            ] {
                let at = verify_code(ram, 0, proof)?;
                require(proof.size >= 16, "Runtime hook lacks a complete entry")?;
                put(ram, at, 0x58000050)?; // LDR X16, literal at +8.
                put(ram, at + 4, 0xd61f0200)?; // BR X16; caller's X30 is retained.
                ram[at + 8..at + 16]
                    .copy_from_slice(&(program_address + symbol as u64).to_le_bytes());
            }
        }
        let mut edits = std::collections::BTreeSet::new();
        for edit in &self.ram_edits {
            require(
                edit.address.is_multiple_of(4)
                    && edits.insert(edit.address)
                    && word(ram, edit.address)? == edit.old,
                "Card RAM edit differs from source",
            )?;
        }
        for edit in &self.ram_edits {
            put(ram, edit.address, edit.new)?;
        }
        // The CRC covers the patched RAM prefix, excluding its namespace.
        put(
            program,
            template::SETUP_SD_REQUEST + 16,
            crate::xp4::crc32(ram),
        )?;
        if let Some(namespace) = namespace {
            ram.resize(namespace_offset, 0);
            ram.extend(namespace);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Compression;

    fn fixture() -> (CardBoot, Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut stock = vec![0; 0x100];
        put(&mut stock, 0x20, 0x94000000).unwrap();
        let proof = |offset: usize| CodeProof {
            address: 0xff800000 + offset,
            size: 4,
            sha256: fujimod_firmware::sha256(&stock[offset..offset + 4]),
        };
        let bytes = vec![0x33; 0x2000];
        let source = |offset, size| Source {
            dat_offset: offset,
            size,
            compression: Compression::Raw,
            decoded_size: size,
            sha256: fujimod_firmware::sha256(&bytes[offset..offset + size]),
            initialized_prefix: None,
        };
        let card = CardBoot {
            hooks: [
                Target::PhyImem,
                Target::PhyDmem,
                Target::LayoutMarker,
                Target::FlashInfo,
            ]
            .into_iter()
            .enumerate()
            .map(|(n, target)| Hook {
                proof: proof(n * 16),
                call: n == 2,
                target,
            })
            .collect(),
            dma: proof(0x40),
            layout_marker: 0x5555aaac,
            jedec_id: 0x2c5b1b,
            secondary_sgi: [0xfa0b0000, 0xfa0d0000, 0xfa0f0000],
            payloads: std::array::from_fn(|n| Payload {
                source: source(n * 32, 32),
                file_offset: n * 32,
                destinations: [0xfb0a0000, 0xfb4a0000],
            }),
            namespace: Some(source(0, bytes.len())),
            unit_cache: None,
            runtime: None,
            ram_edits: vec![],
        };
        (card, stock, bytes.clone(), bytes)
    }

    #[test]
    fn checked_hooks_and_card_bytes_reject_mutated_code_payload_or_binding() {
        for case in 0..10 {
            let (mut card, mut stock, bytes, mut ram) = fixture();
            let mut raw = stock.clone();
            let mut program = template::IMAGE.to_vec();
            match case {
                1 => stock[0x40] ^= 1,
                2 => stock[0] ^= 1,
                3 => put(&mut raw, 0x20, 0).unwrap(),
                4 => card.hooks.pop().map(|_| ()).unwrap(),
                5 => card.hooks.push(card.hooks[0].clone()),
                6 => ram[0] ^= 1,
                7 => card.secondary_sgi[1] = card.secondary_sgi[0],
                8 => program[template::SETUP_SD_CARD_BOOT] ^= 1,
                9 => card.payloads[0].destinations[0] = 0x1000,
                _ => (),
            }
            let result = card.install(
                &bytes,
                None,
                Installation {
                    base: 0xff800000,
                    stock: &stock,
                    program_address: 0xff818000,
                    raw: &mut raw,
                    program: &mut program,
                    ram: &mut ram,
                },
            );
            if case == 0 {
                result.unwrap();
                assert_eq!(&ram[bytes.len()..], bytes);
                assert_eq!(word(&raw, 0x20).unwrap() & 0xfc000000, 0x94000000);
            } else {
                assert!(result.is_err(), "case {case}");
            }
        }
    }

    #[test]
    fn runtime_hooks_pin_all_functions_and_crc_the_patched_ram_prefix() {
        for case in 0..10 {
            let (mut card, stock, bytes, mut ram) = fixture();
            let proof = |address| CodeProof {
                address,
                size: 32,
                sha256: fujimod_firmware::sha256(&bytes[address..address + 32]),
            };
            card.runtime = Some(Runtime {
                read: proof(0x100),
                training_backup: proof(0x140),
                initialize: proof(0x240),
                lock: proof(0x180),
                unlock: proof(0x1c0),
                lock_id: 0x84,
                state: 0x400,
            });
            ram[0x400..0x41c].fill(0);
            card.ram_edits.push(image::Edit {
                address: 0x600,
                old: 0x33333333,
                new: 0x01010101,
                what: "Select card metadata".into(),
            });
            match case {
                1 => ram[0x110] ^= 1,
                2 => ram[0x150] ^= 1,
                3 => ram[0x190] ^= 1,
                4 => ram[0x1d0] ^= 1,
                5 => card.runtime.as_mut().unwrap().lock = proof(0x100),
                6 => ram[0x250] ^= 1,
                7 => ram[0x400] = 1,
                8 => card.ram_edits[0].old ^= 1,
                9 => card.ram_edits.push(card.ram_edits[0].clone()),
                _ => (),
            }
            let mut raw = stock.clone();
            let mut program = template::IMAGE.to_vec();
            let result = card.install(
                &bytes,
                None,
                Installation {
                    base: 0xff800000,
                    stock: &stock,
                    program_address: 0xff818000,
                    raw: &mut raw,
                    program: &mut program,
                    ram: &mut ram,
                },
            );
            if case == 0 {
                result.unwrap();
                assert_eq!(word(&ram, 0x100).unwrap(), 0x58000050);
                assert_eq!(word(&ram, 0x104).unwrap(), 0xd61f0200);
                assert_eq!(
                    u64::from_le_bytes(ram[0x108..0x110].try_into().unwrap()),
                    0xff818000 + template::SETUP_SD_FIRMWARE_READ as u64
                );
                assert_eq!(
                    word(&program, template::SETUP_SD_REQUEST + 16).unwrap(),
                    crate::xp4::crc32(&ram[..bytes.len()])
                );
                assert_eq!(&ram[bytes.len()..], bytes);
            } else {
                assert!(result.is_err(), "case {case}");
            }
        }
    }

    #[test]
    fn retained_unit_cache_cannot_overwrite_code_stack_tables_or_escape_storage() {
        let (mut card, _, _, _) = fixture();
        card.unit_cache = Some(UnitCache {
            storage_bytes: 0x4000,
            source_offset: 0x3000,
            destination_offset: 0x3000,
            size: 0x100,
        });
        let banks = super::super::Banks {
            copies: vec![],
            edits: vec![],
        };
        let storage = vec![0xa5; 0x4000];
        for case in 0..7 {
            let mut changed = card.clone();
            let mut raw = vec![0; 0x4000];
            let cache = changed.unit_cache.as_mut().unwrap();
            match case {
                1 => cache.destination_offset = 0,
                2 => cache.destination_offset = 0x2000,
                3 => cache.destination_offset = 0x2800,
                4 => cache.source_offset = 0x3fff,
                5 => cache.storage_bytes = 0x3000,
                6 => cache.destination_offset = usize::MAX,
                _ => (),
            }
            let result = changed.copy_unit_cache(
                Some(&storage),
                &mut raw,
                0x1000,
                0x2000,
                0x2800..0x3000,
                &banks,
            );
            if case == 0 {
                result.unwrap();
                assert_eq!(&raw[0x3000..0x3100], &storage[0x3000..0x3100]);
            } else {
                assert!(result.is_err(), "case {case}");
            }
        }
    }
}
