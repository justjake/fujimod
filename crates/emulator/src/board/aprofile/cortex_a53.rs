//! Bounded Cortex-A53 implementation-defined configuration registers.
//! DDI 0500J tables 4-17, 4-107/108/109, 4-124/125. No firmware inspection.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registers {
    pub registers: BTreeMap<u16, u64>,
}
fn encoding_name(r: u16) -> String {
    format!(
        "s{}_{}_c{}_c{}_{}",
        r >> 14,
        (r >> 11) & 7,
        (r >> 7) & 15,
        (r >> 3) & 15,
        r & 7
    )
}
impl Registers {
    fn reset(r: u16, cores: usize, gicd: u64) -> Result<u64, String> {
        match r {
            0xcf90 => Ok(0x090c_a000), // DDI 0500J Table 4-17.
            0xcf91 => Ok(0x40),        // TRM reset=0; SMPEN pre-enabled by VM coherency contract.
            0xcf80 => Ok(0x8000_0000), // ACE interface default, no error injection.
            0xce82 if (1..=4).contains(&cores) => Ok(((cores - 1) as u64) << 24), // implementation fields: no ECC, shortest RAM latency.
            0xce83 | 0xcf92 | 0xcf93 => Ok(0), // no error injection; error syndrome invalid/empty.
            0xcf98 => Ok(gicd & 0x0000_00ff_fffc_0000), // PERIPHBASE inferred from boot GICD.
            _ => Err(format!("unmodeled A53 CPU register {}", encoding_name(r))),
        }
    }
    pub fn read(&self, r: u16, cores: usize, gicd: u64) -> Result<u64, String> {
        Self::reset(r, cores, gicd).map(|reset| *self.registers.get(&r).unwrap_or(&reset))
    }
    pub fn write(&mut self, r: u16, value: u64, cores: usize, gicd: u64) -> Result<(), String> {
        let old = self.read(r, cores, gicd)?;
        let next = match r {
            0xcf90 => value & ((1 << 44) | 0x7ffe_e440),
            0xcf91 => value & 0x7f,
            0xcf80 => 0x8000_0000 | (value & 0x2100_4008),
            0xce82 | 0xcf98 => old, // read-only configuration fields; writes ignored.
            0xce83 => (old & value & 0x6000_0000) | (value & 7), // W0C asynchronous errors.
            0xcf92 | 0xcf93 => 0,   // any write clears the complete syndrome.
            _ => unreachable!(),
        };
        self.registers.insert(r, next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a53_fields_and_write_side_effects() {
        let mut s = Registers::default();
        assert_eq!(s.read(0xcf90, 4, 0xfa000000).unwrap(), 0x090ca000);
        assert_eq!(s.read(0xcf91, 4, 0xfa000000).unwrap(), 0x40);
        s.write(0xcf91, u64::MAX, 4, 0xfa000000).unwrap();
        assert_eq!(s.read(0xcf91, 4, 0xfa000000).unwrap(), 0x7f);
        s.write(0xcf90, u64::MAX, 4, 0xfa000000).unwrap();
        assert_eq!(
            s.read(0xcf90, 4, 0xfa000000).unwrap(),
            (1 << 44) | 0x7ffee440
        );
        assert_eq!(s.read(0xce82, 4, 0xfa000000).unwrap(), 3 << 24);
        s.write(0xce82, 0, 4, 0xfa000000).unwrap();
        assert_eq!(s.read(0xce82, 4, 0xfa000000).unwrap(), 3 << 24);
        s.registers.insert(0xce83, 0x60000000);
        s.write(0xce83, 0x20000005, 4, 0xfa000000).unwrap();
        assert_eq!(s.read(0xce83, 4, 0xfa000000).unwrap(), 0x20000005);
        s.registers.insert(0xcf92, 0x8000000080000001);
        s.write(0xcf92, u64::MAX, 4, 0xfa000000).unwrap();
        assert_eq!(s.read(0xcf92, 4, 0xfa000000).unwrap(), 0);
        assert!(s.read(0xdf80, 4, 0xfa000000).is_err());
    }
}
