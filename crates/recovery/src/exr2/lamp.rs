//! SD startup indication with checked stock handlers and GPIO bindings.
use crate::{
    Proof,
    image::{self, Edit, Plan, Result, extent, put, require, word},
    rom_sd::call,
};
use serde::{Deserialize, Serialize};

const PROGRAM: &[u8] = include_bytes!("../../assets/sd-boot-v1-startup-lamp.generated.bin");
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupLamp {
    pub gpio: u32,
    pub pin: u32,
    pub reset_vector: usize,
    pub reset: Proof,
    pub descriptor: Proof,
    pub writer: Proof,
    pub ready: [Proof; 2],
    pub ready_slots: [usize; 2],
    pub writes: Vec<usize>,
    pub gpio_edits: Vec<Edit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Layout {
    schema: usize,
    address: usize,
    size: usize,
    entry: usize,
    writer: usize,
    poll: usize,
    complete: usize,
    config: usize,
}

impl StartupLamp {
    pub(crate) fn install(&self, plan: &Plan, ram: &mut [u8]) -> Result<()> {
        let layout: Layout =
            serde_json::from_str(include_str!("../../assets/sd-boot-v1-startup-lamp.generated.json"))
                .map_err(|e| image::Error::Invalid(e.to_string()))?;
        require(
            layout.schema == 1
                && layout.address == 0x101000
                && layout.size == PROGRAM.len()
                && layout.config + 32 == PROGRAM.len()
                && [layout.entry, layout.writer, layout.poll, layout.complete]
                    .iter()
                    .all(|o| *o < layout.config),
            "Invalid startup lamp layout",
        )?;
        require(
            self.pin < 16
                && (0xfff89000..0xfff89400).contains(&self.gpio)
                && (self.gpio - 0xfff89000).is_multiple_of(0x40)
                && !self.writes.is_empty(),
            "Invalid startup lamp GPIO or writer calls",
        )?;
        for proof in [
            &self.reset,
            &self.descriptor,
            &self.writer,
            &self.ready[0],
            &self.ready[1],
        ] {
            image::hash(extent(ram, proof.address, proof.size)?, &proof.sha256)?;
        }
        require(
            word(ram, self.descriptor.address + 4)? == 1
                && word(ram, self.descriptor.address + 8)?
                    == (((self.gpio - 0xfff89000) / 0x40) << 16 | self.pin)
                && word(ram, self.descriptor.address + 12)? == 0,
            "Startup lamp descriptor is not active-low GPIO",
        )?;
        for region in &plan.regions {
            require(
                layout.address + PROGRAM.len() <= region.address
                    || region.address + region.source.decoded_size <= layout.address,
                "Startup lamp overlaps initialized firmware",
            )?;
        }
        for region in &plan.bss {
            require(
                layout.address + PROGRAM.len() <= region.address
                    || region.address + region.size <= layout.address,
                "Startup lamp overlaps BSS",
            )?;
        }
        require(
            extent(ram, layout.address, PROGRAM.len())?
                .iter()
                .all(|b| *b == 0),
            "Startup lamp reservation is occupied",
        )?;
        require(
            word(ram, self.reset_vector)? as usize == self.reset.address,
            "Startup lamp reset vector differs",
        )?;
        for (slot, ready) in self.ready_slots.iter().zip(&self.ready) {
            require(
                word(ram, *slot)? as usize == ready.address,
                "Startup completion handler differs",
            )?;
        }
        for site in &self.writes {
            require(
                call(ram, *site)? == self.writer.address,
                "Startup lamp writer call differs",
            )?;
        }
        for edit in &self.gpio_edits {
            require(
                word(ram, edit.address)? == edit.old,
                "Startup lamp GPIO defaults differ",
            )?;
        }
        let mut program = PROGRAM.to_vec();
        for (i, value) in [
            self.gpio,
            self.pin,
            self.reset.address as u32,
            self.descriptor.address as u32,
            self.writer.address as u32,
            self.ready[0].address as u32,
            self.ready[1].address as u32,
            1,
        ]
        .iter()
        .enumerate()
        {
            put(&mut program, layout.config + i * 4, *value)?;
        }
        ram[layout.address..layout.address + PROGRAM.len()].copy_from_slice(&program);
        put(
            ram,
            self.reset_vector,
            (layout.address + layout.entry) as u32,
        )?;
        for (slot, offset) in self.ready_slots.iter().zip([layout.poll, layout.complete]) {
            put(ram, *slot, (layout.address + offset) as u32)?;
        }
        for site in &self.writes {
            let delta = (layout.address + layout.writer) as i64 - *site as i64 - 8;
            require(
                delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
                "Startup lamp BL exceeds ARM range",
            )?;
            put(ram, *site, 0xeb000000 | ((delta >> 2) as u32 & 0xffffff))?;
        }
        for edit in &self.gpio_edits {
            put(ram, edit.address, edit.new)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{Compression, Region, Source};

    fn fixture() -> (StartupLamp, Plan, Vec<u8>) {
        let mut ram = vec![0; 0x210000];
        for (at, value) in [
            (0x20, 0x1800),
            (0x200104, 1),
            (0x200108, 0xf0002),
            (0x200080, 0xebffffde),
            (0x200500, 0x200300),
            (0x200504, 0x200400),
            (0x1900, 0xffff0004),
        ] {
            put(&mut ram, at, value).unwrap();
        }
        let proof = |address, size| Proof {
            address,
            size,
            sha256: fujimod_firmware::sha256(&ram[address..address + size]),
        };
        let lamp = StartupLamp {
            gpio: 0xfff893c0,
            pin: 2,
            reset_vector: 0x20,
            reset: proof(0x1800, 4),
            descriptor: proof(0x200100, 28),
            writer: proof(0x200000, 4),
            ready: [proof(0x200300, 4), proof(0x200400, 4)],
            ready_slots: [0x200500, 0x200504],
            writes: vec![0x200080],
            gpio_edits: vec![Edit {
                address: 0x1900,
                old: 0xffff0004,
                new: 0xffff0000,
                what: "Green startup output".into(),
            }],
        };
        let plan = Plan {
            size: ram.len(),
            entry: 0,
            bss: vec![],
            edits: vec![],
            regions: vec![Region {
                name: "application".into(),
                address: 0x200000,
                source: Source {
                    dat_offset: 0,
                    size: 0x1000,
                    compression: Compression::Raw,
                    decoded_size: 0x1000,
                    sha256: String::new(),
                    initialized_prefix: None,
                },
            }],
        };
        (lamp, plan, ram)
    }

    #[test]
    fn preserves_stock_handlers_and_checks_every_binding_before_installing() {
        let (lamp, plan, ram) = fixture();
        let mut installed = ram.clone();
        lamp.install(&plan, &mut installed).unwrap();
        assert_ne!(word(&installed, 0x20).unwrap(), 0x1800);
        assert_ne!(word(&installed, 0x200500).unwrap(), 0x200300);
        assert_eq!(&installed[0x200000..0x200004], &ram[0x200000..0x200004]);
        assert_eq!(word(&installed, 0x1900).unwrap(), 0xffff0000);
        for at in [
            0x20, 0x1800, 0x1900, 0x200000, 0x200080, 0x200104, 0x200300, 0x200400, 0x200500,
            0x101000,
        ] {
            let mut changed = ram.clone();
            changed[at] ^= 1;
            let before = changed.clone();
            assert!(lamp.install(&plan, &mut changed).is_err(), "{at:x}");
            assert_eq!(changed, before, "rejected input changed at {at:x}");
        }
    }
}
