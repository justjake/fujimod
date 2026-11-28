//! ARMv7-R memory protection. The highest-numbered matching PMSAv7 region
//! supplies the access permissions and memory attributes for its address.
use crate::Access;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub base: u32,
    pub size: u32,
    pub attributes: u32,
}

impl Region {
    fn geometry(self) -> Option<(u64, u64)> {
        let bits = ((self.size >> 1) & 31) + 1;
        if self.size & 1 == 0 || bits < 5 {
            return None;
        }
        let bytes = 1u64 << bits;
        let base = u64::from(self.base) & !(bytes - 1);
        Some((base, bytes))
    }

    /// Region and subregion boundaries, including the exclusive upper bound.
    pub fn boundaries(self) -> impl Iterator<Item = u64> {
        self.geometry().into_iter().flat_map(|(base, bytes)| {
            let parts = if bytes >= 256 { 8 } else { 1 };
            (0..=parts).map(move |part| base + part * bytes / parts)
        })
    }

    fn contains(self, address: u32) -> bool {
        let Some((base, bytes)) = self.geometry() else {
            return false;
        };
        let offset = u64::from(address).wrapping_sub(base);
        if offset >= bytes {
            return false;
        }
        bytes < 256 || (self.size >> (8 + offset / (bytes / 8))) & 1 == 0
    }

    fn allows(self, access: Access, user: bool) -> bool {
        let ap = (self.attributes >> 8) & 7;
        let read = matches!(ap, 2 | 3 | 6) || !user && matches!(ap, 1 | 5);
        match access {
            Access::Read => read,
            Access::Write => ap == 3 || !user && matches!(ap, 1 | 2),
            Access::Fetch => read && self.attributes & (1 << 12) == 0,
        }
    }
}

pub fn matching_region(regions: &[Region], address: u32) -> Option<Region> {
    regions
        .iter()
        .rev()
        .copied()
        .find(|region| region.contains(address))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectionFault {
    Background,
    Permission,
}

/// Check an address against the MPU. Addresses stay unchanged by the MPU.
/// An enabled region also shadows the background for denied accesses.
pub fn check(
    sctlr: u32,
    regions: &[Region],
    address: u32,
    access: Access,
    user: bool,
) -> Result<(), ProtectionFault> {
    let mpu_enabled = sctlr & 1 != 0;
    let region = mpu_enabled
        .then(|| matching_region(regions, address))
        .flatten();
    let allowed = if let Some(region) = region {
        region.allows(access, user)
    } else {
        let background_enabled = sctlr & (1 << 17) != 0;
        if mpu_enabled && (user || !background_enabled) {
            return Err(ProtectionFault::Background);
        }
        let high_vectors = sctlr & (1 << 13) != 0;
        access != Access::Fetch || address < 0x8000_0000 || high_vectors && address >= 0xf000_0000
    };
    allowed.then_some(()).ok_or(ProtectionFault::Permission)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_overlapping_region_wins_only_in_its_enabled_subregions() {
        let regions = [
            Region {
                base: 0,
                size: 0x3f,
                attributes: 3 << 8,
            },
            Region {
                base: 0x1000,
                size: 0x17 | (1 << 8),
                attributes: (2 << 8) | (1 << 12),
            },
        ];
        assert_eq!(matching_region(&regions, 0x1000), Some(regions[0]));
        assert_eq!(matching_region(&regions, 0x1200), Some(regions[1]));
        assert_eq!(matching_region(&regions, 0x2000), Some(regions[0]));
        assert_eq!(
            regions[1].boundaries().collect::<Vec<_>>(),
            (0x1000..=0x2000).step_by(0x200).collect::<Vec<_>>()
        );
        assert_eq!(check(1, &regions, 0x1000, Access::Fetch, true), Ok(()));
        assert_eq!(
            check(1, &regions, 0x1200, Access::Fetch, false),
            Err(ProtectionFault::Permission)
        );
        assert_eq!(check(1, &regions, 0x1200, Access::Read, true), Ok(()));
        assert_eq!(
            check(1, &regions, 0x1200, Access::Write, true),
            Err(ProtectionFault::Permission)
        );
        assert_eq!(check(1, &regions, 0x1200, Access::Write, false), Ok(()));
    }

    #[test]
    fn background_permissions_distinguish_modes_and_high_vectors() {
        assert_eq!(
            check(1, &[], 0, Access::Read, false),
            Err(ProtectionFault::Background)
        );
        let background = 1 | (1 << 17);
        assert_eq!(
            check(background, &[], 0, Access::Read, true),
            Err(ProtectionFault::Background)
        );
        assert_eq!(check(background, &[], 0, Access::Read, false), Ok(()));
        assert_eq!(
            check(0, &[], 0xffff_0000, Access::Fetch, false),
            Err(ProtectionFault::Permission)
        );
        assert_eq!(
            check(1 << 13, &[], 0xffff_0000, Access::Fetch, false),
            Ok(())
        );
    }
}
