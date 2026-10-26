//! Per-core tightly coupled RAM. Region size is read-only; moving a region
//! changes the window onto the same private bytes (ARM DDI0363E 4.2.21/22).
use fujimod_emulator_cpu::memory::Backing;
use std::cell::Cell;

#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BankOptions {
    pub bytes: usize,
    pub reset: u32,
}
#[derive(Clone, Copy, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub btcm: BankOptions,
    pub atcm: BankOptions,
}
pub fn normalize(bytes: usize, word: u32) -> Result<u32, String> {
    if bytes == 0 {
        if word & 1 != 0 {
            return Err("cannot enable an absent TCM interface".into());
        }
        return Ok(word & 0xfffff000);
    }
    if !(0x1000..=0x800000).contains(&bytes) || !bytes.is_power_of_two() {
        return Err("TCM physical size must be zero or a power of two from 4 KiB to 8 MiB".into());
    }
    Ok((word & !(bytes as u32 - 1)) | ((bytes.trailing_zeros() - 9) << 2) | (word & 1))
}
pub(crate) fn validate_disjoint(
    first_size: usize,
    first: u32,
    second_size: usize,
    second: u32,
) -> Result<(), String> {
    if first & 1 != 0 && second & 1 != 0 {
        let a = (first & 0xfffff000) as u64;
        let b = (second & 0xfffff000) as u64;
        if a < b + second_size as u64 && b < a + first_size as u64 {
            return Err("ATCM and BTCM must not overlap".into());
        }
    }
    Ok(())
}
pub(super) struct Bank {
    pub memory: Backing,
    pub register: Cell<u32>,
    pub tracker: fujimod_emulator_cpu::memory::pages::Tracker,
}
impl Bank {
    pub fn extent(&self) -> Option<(u64, usize)> {
        let r = self.register.get();
        (r & 1 != 0 && self.memory.len() != 0)
            .then_some(((r & 0xfffff000) as u64, self.memory.len()))
    }
}
pub(super) fn banks(options: Options) -> Result<[Bank; 2], String> {
    let specs = [options.btcm, options.atcm];
    let regs = [
        normalize(specs[0].bytes, specs[0].reset)?,
        normalize(specs[1].bytes, specs[1].reset)?,
    ];
    validate_disjoint(specs[0].bytes, regs[0], specs[1].bytes, regs[1])?;
    let make = |i: usize| -> Result<Bank, String> {
        let bytes = specs[i].bytes;
        Ok(Bank {
            memory: if bytes == 0 {
                Backing::zeros(0)
            } else {
                Backing::aligned_zeros(bytes, 4096)?
            },
            register: Cell::new(regs[i]),
            tracker: fujimod_emulator_cpu::memory::pages::Tracker::new(bytes),
        })
    };
    Ok([make(0)?, make(1)?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn size_alignment_absent_interfaces_and_overlap_are_checked_before_mapping() {
        assert_eq!(normalize(0x2000, 0xffff8411).unwrap(), 0xffff8011);
        assert_eq!(normalize(0x2000, 0x7000007d).unwrap(), 0x70000011);
        assert!(normalize(0, 1).is_err());
        assert!(normalize(0x1800, 0).is_err());
        assert!(normalize(0x1000000, 0).is_err());
        assert!(
            banks(Options {
                btcm: BankOptions {
                    bytes: 0x2000,
                    reset: 0x70000001
                },
                atcm: BankOptions {
                    bytes: 0x2000,
                    reset: 0x70000001
                }
            })
            .is_err()
        );
        assert!(
            banks(Options {
                btcm: BankOptions {
                    bytes: 0x2000,
                    reset: 0x70000001
                },
                atcm: BankOptions {
                    bytes: 0x2000,
                    reset: 0x70002001
                }
            })
            .is_ok()
        );
    }
}
