//! Configuration defaults for the startup fallback, sourced from the card.
//!
//! When the configuration backup fails its signature, startup zeroes the
//! configuration buffer and fills it from the defaults image the update
//! installed into internal flash. The card appends that image after the RAM
//! image and turns the flash read into the firmware's own byte copy from it.
use crate::{
    Proof,
    image::{self, Plan, Result, Source, extent, put, require, word},
    rom_sd::call,
};
use serde::{Deserialize, Serialize};

/// `mov r1, #0`: the defaults image's flash offset.
const OFFSET_ZERO: u32 = 0xe3a01000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationDefaults {
    pub check: Proof,
    pub offset_load: usize,
    pub read_call: usize,
    pub read_target: usize,
    pub copy: Proof,
    pub staging: usize,
    pub source: Source,
}

/// `mov r1, #value` when `value` is an A32 rotated immediate.
fn mov_r1(value: u32) -> Option<u32> {
    (0..16).find_map(|rotation| {
        let imm = value.rotate_left(rotation * 2);
        (imm <= 0xff).then_some(0xe3a01000 | rotation << 8 | imm)
    })
}

fn bl(at: usize, target: usize) -> Result<u32> {
    let delta = target as i64 - at as i64 - 8;
    require(
        delta % 4 == 0 && (-0x2000000..0x2000000).contains(&delta),
        "Configuration defaults copy BL exceeds ARM range",
    )?;
    Ok(0xeb000000 | ((delta >> 2) as u32 & 0xffffff))
}

impl ConfigurationDefaults {
    pub(crate) fn install(&self, inverted: &[u8], plan: &Plan, ram: &mut Vec<u8>) -> Result<()> {
        for proof in [&self.check, &self.copy] {
            image::hash(extent(ram, proof.address, proof.size)?, &proof.sha256)?;
        }
        require(
            self.offset_load + 8 == self.read_call
                && self.offset_load >= self.check.address
                && self.read_call + 4 <= self.check.address + self.check.size
                && word(ram, self.offset_load)? == OFFSET_ZERO
                && call(ram, self.read_call)? == self.read_target,
            "Configuration defaults read differs",
        )?;
        let defaults = self.source.read(inverted)?;
        require(
            defaults.get(0x202..0x204) == Some(&[0x55, 0xaa][..]),
            "Update configuration defaults lack their signature",
        )?;
        let end = self.staging + defaults.len();
        require(
            self.staging >= ram.len()
                && plan.bss.iter().all(|b| end <= b.address || b.address + b.size <= self.staging)
                && end < 0x40000000,
            "Configuration defaults staging overlaps the image",
        )?;
        let load = u32::try_from(self.staging)
            .ok()
            .and_then(mov_r1)
            .ok_or_else(|| {
                image::Error::Invalid("Defaults staging is not an A32 immediate".into())
            })?;
        let copy = bl(self.read_call, self.copy.address)?;
        put(ram, self.offset_load, load)?;
        put(ram, self.read_call, copy)?;
        ram.resize(self.staging, 0);
        ram.extend_from_slice(&defaults);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Compression;

    fn fixture() -> (ConfigurationDefaults, Plan, Vec<u8>, Vec<u8>) {
        let mut defaults = vec![0x5a; 0x400];
        defaults[0x202..0x204].copy_from_slice(&[0x55, 0xaa]);
        let mut ram = vec![0; 0x3000];
        for (at, value) in [
            (0x1004, OFFSET_ZERO),
            (0x1008, 0xe3a02b01),
            (0x100c, 0xeb0003fb),
        ] {
            put(&mut ram, at, value).unwrap();
        }
        put(&mut ram, 0x2800, 0xe12fff1e).unwrap();
        let proof = |ram: &[u8], address, size| Proof {
            address,
            size,
            sha256: fujimod_firmware::sha256(&ram[address..address + size]),
        };
        let value = ConfigurationDefaults {
            check: proof(&ram, 0x1000, 0x20),
            offset_load: 0x1004,
            read_call: 0x100c,
            read_target: 0x2000,
            copy: proof(&ram, 0x2800, 4),
            staging: 0x8000,
            source: Source {
                dat_offset: 0,
                size: defaults.len(),
                compression: Compression::Raw,
                decoded_size: defaults.len(),
                sha256: fujimod_firmware::sha256(&defaults),
                initialized_prefix: None,
            },
        };
        let plan = Plan { size: ram.len(), entry: 0, regions: vec![], bss: vec![], edits: vec![] };
        (value, plan, defaults, ram)
    }

    #[test]
    fn copies_the_appended_defaults_instead_of_reading_flash() {
        let (value, plan, defaults, ram) = fixture();
        let mut result = ram.clone();
        value.install(&defaults, &plan, &mut result).unwrap();
        assert_eq!(word(&result, 0x1004).unwrap(), 0xe3a01902);
        assert_eq!(call(&result, 0x100c).unwrap(), 0x2800);
        assert_eq!(&result[0x8000..], &defaults[..]);
        assert!(result[ram.len()..0x8000].iter().all(|b| *b == 0));
        assert_eq!(&result[..0x1004], &ram[..0x1004]);
        assert_eq!(&result[0x1010..ram.len()], &ram[0x1010..]);
    }

    #[test]
    fn changed_code_source_or_staging_refuses() {
        let (value, plan, defaults, ram) = fixture();
        for at in [0x1004, 0x100c, 0x2800] {
            let mut changed = ram.clone();
            changed[at] ^= 1;
            assert!(value.install(&defaults, &plan, &mut changed).is_err());
        }
        let mut unsigned = defaults.clone();
        unsigned[0x202] = 0;
        let mut altered = value.clone();
        altered.source.sha256 = fujimod_firmware::sha256(&unsigned);
        assert!(altered.install(&unsigned, &plan, &mut ram.clone()).is_err());
        for staging in [0x2000, 0x8004] {
            let mut altered = value.clone();
            altered.staging = staging;
            assert!(altered.install(&defaults, &plan, &mut ram.clone()).is_err());
        }
    }
}
