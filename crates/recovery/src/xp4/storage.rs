//! Checked firmware-reader overrides and immutable card backing.
use super::sd_loader;
use crate::{
    Proof,
    image::{self, Plan, Result, extent, hash, put, require, word},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    pub address_limit: usize,
    pub backing_address: usize,
    pub capacity: usize,
    pub dat_offset: usize,
    pub enabled_byte: usize,
    pub byte_limit: usize,
    pub primary_call: Proof,
    pub byte_reader: Proof,
    pub indexed_reader: Proof,
    pub cache_maintenance: Proof,
}

fn branch(at: usize, target: usize, opcode: u32) -> Result<u32> {
    let delta = target as i64 - at as i64 - 8;
    require(
        delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
        "Card transport hook is out of ARM branch range",
    )?;
    Ok(opcode | ((delta >> 2) as u32 & 0xffffff))
}

impl Transport {
    pub(super) fn install(&self, inverted: &[u8], plan: &Plan, ram: &mut Vec<u8>) -> Result<()> {
        let layout = &sd_loader::LAYOUT.card;
        let program_base = layout.address;
        let program_bytes = &sd_loader::IMAGE[layout.offset..layout.offset + layout.size];
        let program_end = program_base + program_bytes.len();
        let source = extent(
            inverted,
            self.dat_offset,
            inverted.len().checked_sub(self.dat_offset).ok_or_else(|| {
                image::Error::Invalid("Card firmware source offset is outside DAT".into())
            })?,
        )?;
        let tail = (ram.len() + 3) & !3;
        require(
            self.byte_limit > 0
                && self.byte_limit <= self.capacity
                && source.len() <= self.capacity
                && self.capacity <= 0x8000000
                && self.backing_address % 4 == 0
                && self
                    .backing_address
                    .checked_add(self.capacity)
                    .is_some_and(|end| end <= self.address_limit)
                && self.address_limit <= u32::MAX as usize
                && tail
                    .checked_add(source.len())
                    .is_some_and(|end| end <= self.backing_address),
            "Card firmware backing escapes declared RAM or overlaps its source",
        )?;
        for region in &plan.regions {
            require(
                program_end <= region.address
                    || region.address + region.source.decoded_size <= program_base,
                "Card transport overlaps initialized firmware",
            )?;
        }
        for region in &plan.bss {
            require(
                program_end <= region.address || region.address + region.size <= program_base,
                "Card transport overlaps firmware BSS",
            )?;
        }
        require(
            extent(ram, program_base, program_bytes.len())?
                .iter()
                .all(|b| *b == 0),
            "Card transport reservation is occupied",
        )?;
        for proof in [
            &self.primary_call,
            &self.byte_reader,
            &self.indexed_reader,
            &self.cache_maintenance,
        ] {
            let end = proof.address.checked_add(proof.size).ok_or_else(|| {
                image::Error::Invalid("Card firmware hook extent overflows".into())
            })?;
            require(
                proof.address % 4 == 0
                    && proof.size > 0
                    && (end <= program_base || program_end <= proof.address),
                "Card firmware hook overlaps its program",
            )?;
            hash(extent(ram, proof.address, proof.size)?, &proof.sha256)?;
        }
        require(
            self.primary_call.size == 4 && self.primary_call.address >= 4,
            "Invalid primary loader hook",
        )?;
        let original = word(ram, self.primary_call.address)?;
        require(
            original >> 24 == 0x0b && word(ram, self.primary_call.address - 4)? == 0xe3500000,
            "Card firmware copy must precede the conditional primary initialization call",
        )?;
        let primary = self.primary_call.address as i64
            + 8
            + (((original & 0xffffff) as i32) << 8 >> 6) as i64;
        require(
            primary >= 0 && primary < 0x40000,
            "Primary initialization escapes loader",
        )?;
        let mut program = program_bytes.to_vec();
        let config = layout.symbols["card_firmware_config"];
        for (index, value) in [
            tail,
            self.backing_address,
            source.len(),
            self.capacity,
            self.enabled_byte,
            self.byte_limit,
            primary as usize,
            self.cache_maintenance.address,
        ]
        .into_iter()
        .enumerate()
        {
            put(
                &mut program,
                config + index * 4,
                u32::try_from(value).map_err(|_| {
                    image::Error::Invalid("Card transport configuration overflows ARM32".into())
                })?,
            )?;
        }
        let call = branch(
            self.primary_call.address,
            program_base + layout.symbols["card_firmware_bootstrap"],
            original & 0xff000000,
        )?;
        let byte = branch(
            self.byte_reader.address,
            program_base + layout.symbols["card_firmware_bytes"],
            0xea000000,
        )?;
        let indexed = branch(
            self.indexed_reader.address,
            program_base + layout.symbols["card_firmware_indexed"],
            0xea000000,
        )?;
        ram[program_base..program_end].copy_from_slice(&program);
        put(ram, self.primary_call.address, call)?;
        put(ram, self.byte_reader.address, byte)?;
        put(ram, self.indexed_reader.address, indexed)?;
        ram.resize(tail, 0);
        ram.extend_from_slice(source);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Transport, Vec<u8>, Plan) {
        let mut ram = vec![0; sd_loader::LAYOUT.card.address + 0x1000];
        put(&mut ram, 0x150, 0xe3500000).unwrap();
        put(&mut ram, 0x154, 0x0b0003fe).unwrap();
        let proof = |address, size| Proof {
            address,
            size,
            sha256: fujimod_firmware::sha256(&ram[address..address + size]),
        };
        let transport = Transport {
            address_limit: 0x20000000,
            backing_address: 0x1c000000,
            capacity: 0x4000000,
            dat_offset: 8,
            enabled_byte: 0x900,
            byte_limit: 0x2fe0000,
            primary_call: proof(0x154, 4),
            byte_reader: proof(0x400, 68),
            indexed_reader: proof(0x500, 228),
            cache_maintenance: proof(0x700, 40),
        };
        let plan = Plan {
            size: ram.len(),
            entry: 0,
            regions: vec![],
            bss: vec![],
            edits: vec![],
        };
        (transport, ram, plan)
    }

    #[test]
    fn immutable_payload_and_primary_condition_are_preserved() {
        let (transport, mut ram, plan) = fixture();
        let tail = ram.len();
        let input: Vec<u8> = (0..128).collect();
        transport.install(&input, &plan, &mut ram).unwrap();
        assert_eq!(&ram[tail..], &input[8..]);
        assert_eq!(word(&ram, 0x154).unwrap() >> 24, 0x0b);
        assert_eq!(word(&ram, 0x400).unwrap() >> 24, 0xea);
        assert_eq!(word(&ram, 0x500).unwrap() >> 24, 0xea);
        assert_eq!(
            word(
                &ram,
                sd_loader::LAYOUT.card.address + sd_loader::LAYOUT.card.symbols["card_firmware_config"]
            )
            .unwrap(),
            tail as u32
        );
    }

    #[test]
    fn occupied_program_and_escaping_backing_refuse() {
        let (mut transport, mut ram, plan) = fixture();
        transport.backing_address = transport.address_limit - 4;
        assert!(transport.install(&[0; 128], &plan, &mut ram).is_err());
        let (transport, mut ram, plan) = fixture();
        ram[sd_loader::LAYOUT.card.address] = 1;
        assert!(transport.install(&[0; 128], &plan, &mut ram).is_err());
        let (transport, mut ram, mut plan) = fixture();
        plan.bss.push(image::Bss {
            name: "occupied".into(),
            address: sd_loader::LAYOUT.card.address,
            size: 4096,
        });
        assert!(transport.install(&[0; 128], &plan, &mut ram).is_err());
    }

    #[test]
    fn changed_reader_identity_and_secondary_call_refuse() {
        let (transport, mut ram, plan) = fixture();
        ram[0x404] = 1;
        assert!(transport.install(&[0; 128], &plan, &mut ram).is_err());
        let (mut transport, mut ram, plan) = fixture();
        put(&mut ram, 0x154, 0xeb0003fe).unwrap();
        transport.primary_call.sha256 = fujimod_firmware::sha256(&ram[0x154..0x158]);
        assert!(transport.install(&[0; 128], &plan, &mut ram).is_err());
    }
}
