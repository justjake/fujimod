//! Architectural GIC register windows, including the banked GICv2 CPU interface.
use fujimod_emulator_devices::gic::InterruptController;

pub enum InterruptRegisters {
    GicV2 {
        distributor: u64,
        cpu_interface: u64,
    },
    GicV3 {
        distributor: u64,
        redistributor: u64,
        stride: u64,
    },
}
pub struct InterruptMap {
    pub gic: Box<dyn InterruptController>,
    pub registers: InterruptRegisters,
    pub cores: usize,
}
impl InterruptMap {
    pub(super) fn validate(&self, processors: usize) -> Result<(), String> {
        if self.cores == 0 || self.cores > processors {
            return Err("invalid GIC processor map".into());
        }
        let (first, first_size, second, second_size) = match self.registers {
            InterruptRegisters::GicV2 {
                distributor,
                cpu_interface,
            } => (distributor, 0x1000, cpu_interface, 0x1000),
            InterruptRegisters::GicV3 {
                distributor,
                redistributor,
                stride,
            } => {
                if stride < 0x20000 || !stride.is_multiple_of(0x10000) {
                    return Err("invalid GIC redistributor stride".into());
                }
                (
                    distributor,
                    0x10000,
                    redistributor,
                    stride
                        .checked_mul(self.cores as u64)
                        .ok_or("GIC geometry overflow")?,
                )
            }
        };
        let end = first
            .checked_add(first_size)
            .ok_or("GIC geometry overflow")?;
        let second_end = second
            .checked_add(second_size)
            .ok_or("GIC geometry overflow")?;
        if first & 0xfff != 0 || second & 0xfff != 0 || first < second_end && second < end {
            return Err("invalid GIC address geometry".into());
        }
        for core in 0..self.cores {
            self.gic.pins(core)?;
        }
        Ok(())
    }
    pub(super) fn access(
        &mut self,
        core: usize,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Option<Result<u64, String>> {
        let (distributor, extent) = match self.registers {
            InterruptRegisters::GicV2 { distributor, .. } => (distributor, 0x1000),
            InterruptRegisters::GicV3 { distributor, .. } => (distributor, 0x10000),
        };
        if (distributor..distributor + extent).contains(&address) {
            return Some(
                self.gic
                    .distributor(core, address - distributor, size, value),
            );
        }
        match self.registers {
            InterruptRegisters::GicV2 { cpu_interface, .. } => {
                if (cpu_interface..cpu_interface + 0x1000).contains(&address) {
                    return Some(self.gic.cpu_interface(
                        core,
                        address - cpu_interface,
                        size,
                        value,
                    ));
                }
            }
            InterruptRegisters::GicV3 {
                redistributor,
                stride,
                ..
            } => {
                if (redistributor..redistributor + stride * self.cores as u64).contains(&address) {
                    let offset = address - redistributor;
                    return Some(self.gic.redistributor(
                        (offset / stride) as usize,
                        offset % stride,
                        size,
                        value,
                    ));
                }
            }
        }
        None
    }
    /// Construct a relocated GIC-v3 distributor and redistributor block.
    ///
    /// The bounds mirror `fujimod_emulator_ip_arm::gicv3::Gic::new`, so they
    /// are validated here first and reported as errors. Address geometry is
    /// validated against a one-core board; the board constructor revalidates
    /// the core count against its processors.
    pub fn gic_v3(
        distributor: u64,
        redistributor: u64,
        stride: u64,
        cores: usize,
        lines: usize,
    ) -> Result<Self, String> {
        if cores == 0 || cores > 256 {
            return Err(format!("GICv3 core count {cores} outside 1..=256"));
        }
        if !(32..=1020).contains(&lines) {
            return Err(format!("GICv3 line count {lines} outside 32..=1020"));
        }
        let map = InterruptMap {
            gic: Box::new(fujimod_emulator_ip_arm::gicv3::Gic::new(
                (0..cores as u32).collect(),
                lines,
            )?),
            registers: InterruptRegisters::GicV3 {
                distributor,
                redistributor,
                stride,
            },
            cores,
        };
        map.validate(cores)?;
        Ok(map)
    }
    /// Identity-mapped MMIO windows this map serves, as (address, size).
    pub fn windows(&self) -> Vec<(u64, u64)> {
        match self.registers {
            InterruptRegisters::GicV2 {
                distributor,
                cpu_interface,
            } => vec![(distributor, 0x1000), (cpu_interface, 0x1000)],
            InterruptRegisters::GicV3 {
                distributor,
                redistributor,
                stride,
            } => vec![
                (distributor, 0x10000),
                (redistributor, stride * self.cores as u64),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_ip_arm::gicv2::Gic;
    #[test]
    fn relocated_v2_windows_preserve_banked_cpu_state_and_sgi_source() {
        let mut map = InterruptMap {
            gic: Box::new(Gic::new(2, 64)),
            cores: 2,
            registers: InterruptRegisters::GicV2 {
                distributor: 0x10000,
                cpu_interface: 0x18000,
            },
        };
        map.validate(2).unwrap();
        let write = |map: &mut InterruptMap, core, address, value| {
            map.access(core, address, 4, Some(value)).unwrap().unwrap();
        };
        write(&mut map, 0, 0x10000, 1);
        write(&mut map, 1, 0x18000, 1);
        write(&mut map, 1, 0x18004, 255);
        assert_eq!(map.access(0, 0x18000, 4, None).unwrap().unwrap(), 0);
        write(&mut map, 0, 0x10f00, 0x00020003);
        assert_eq!(map.gic.pins(0).unwrap(), (false, false));
        assert_eq!(map.gic.pins(1).unwrap(), (true, false));
        assert_eq!(map.access(1, 0x1800c, 4, None).unwrap().unwrap(), 3);
        assert_eq!(map.gic.pins(1).unwrap(), (false, false));
        write(&mut map, 1, 0x18010, 3);
        assert_eq!(map.access(1, 0x1800c, 4, None).unwrap().unwrap(), 1023);
        assert!(map.access(0, 0x11000, 4, None).is_none());
        assert!(map.access(0, 0x19000, 4, None).is_none());
        assert!(map.access(2, 0x18000, 4, None).unwrap().is_err());
        map.registers = InterruptRegisters::GicV2 {
            distributor: 0x10000,
            cpu_interface: 0x10000,
        };
        assert!(map.validate(2).is_err());
    }
    #[test]
    fn gic_v3_constructor_reports_bounds_and_geometry() {
        let map = InterruptMap::gic_v3(0xfa00_0000, 0xfa08_0000, 0x20000, 4, 512).unwrap();
        assert_eq!(map.cores, 4);
        assert_eq!(
            map.windows(),
            vec![(0xfa00_0000, 0x10000), (0xfa08_0000, 0x80000)]
        );
        assert!(InterruptMap::gic_v3(0, 0x10000, 0x20000, 0, 512).is_err());
        assert!(InterruptMap::gic_v3(0, 0x10000, 0x20000, 257, 512).is_err());
        assert!(InterruptMap::gic_v3(0, 0x10000, 0x20000, 4, 31).is_err());
        assert!(InterruptMap::gic_v3(0, 0x10000, 0x20000, 4, 1021).is_err());
        assert!(InterruptMap::gic_v3(0x10000, 0x20000, 0x10000, 4, 512).is_err());
        assert!(InterruptMap::gic_v3(1, 0x20000, 0x20000, 4, 512).is_err());
    }
}
