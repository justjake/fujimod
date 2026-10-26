//! Located storage and readable processor-state facts from mapped firmware.
//! Names nominate candidates; decoded instructions and mapped tables verify them.
mod exr2;
mod exr2_execution;
pub use exr2_execution::exr2_execution;
mod flash;
mod update;
pub use update::flash_update;
mod reset;
pub use exr2::{exr2_control, exr2_flash};
pub use flash::{flash, image_offsets};
pub use reset::{ResetAudit, ResetCall, ResetVector, boot_rom_audit};
use serde::{Deserialize, Serialize};

/// A CPU-visible region safe for sequential state capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRegion {
    pub name: String,
    pub address: u64,
    pub size: u64,
    /// Read twice and require equal bytes.
    pub constant: bool,
}

/// Confirmed processor-family boot ROM extent. An absent extent is not inferred
/// from retained pointers: those may address ROM that cannot be read after boot.
pub fn boot_rom(processor: &str) -> Result<StateRegion, String> {
    match processor {
        "exr2" => Ok(StateRegion {
            name: "boot-rom".into(),
            address: 0x8000_0000,
            size: 0x4000,
            constant: true,
        }),
        _ => Err(format!(
            "{processor}: no established safely readable boot ROM extent"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_rom_is_family_scoped_and_does_not_promote_rom_pointers() {
        let rom = boot_rom("exr2").unwrap();
        assert_eq!(
            (rom.address, rom.size, rom.constant),
            (0x80000000, 16384, true)
        );
        for generation in ["exr", "exrpro", "xpp", "xp4", "xp5", "unknown"] {
            assert!(boot_rom(generation).is_err());
        }
    }
}
