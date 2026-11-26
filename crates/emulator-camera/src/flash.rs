//! The catalog's stage-1 internal-flash blocks as one saved image.
//!
//! An autostart install rewrites the blocks the boot ROM loads; this is how
//! they are read out and programmed back into a camera's NAND.
use crate::{OpenError, Stage};
use fujimod_catalog::nand_boot::FirmwareNandBoot;
use fujimod_emulator_session::Session;

/// The stage-1 blocks as the kept internal flash: each block's pages, then its
/// spares, in the order the boot ROM loads them.
pub fn read(session: &Session, nand: &FirmwareNandBoot) -> Result<Vec<u8>, OpenError> {
    let medium = session
        .nand_medium("nand")
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    let page = medium.options().page_bytes as u64;
    let pages = medium.options().pages_per_block;
    let spare_bytes = medium.options().spare_bytes;
    let mut bytes = Vec::new();
    let mut spare = vec![0u8; spare_bytes];
    for block in &nand.stage1.blocks {
        let first = block.native_offset / page;
        let mut data = vec![0u8; block.bytes as usize];
        medium
            .read(block.native_offset, &mut data)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        bytes.extend_from_slice(&data);
        for index in 0..pages {
            medium
                .read_spare((first + index as u64) as u32, &mut spare)
                .map_err(|error| OpenError::at(Stage::BootRom, error))?;
            bytes.extend_from_slice(&spare);
        }
    }
    Ok(bytes)
}

/// Program a saved internal flash image into the camera's NAND.
pub fn write(
    session: &mut Session,
    nand: &FirmwareNandBoot,
    bytes: &[u8],
) -> Result<(), OpenError> {
    let mut medium = session
        .nand_medium("nand")
        .map_err(|error| OpenError::at(Stage::BootRom, error))?;
    let page = medium.options().page_bytes as u64;
    let pages = medium.options().pages_per_block as usize;
    let spare_bytes = medium.options().spare_bytes;
    let expected: usize = nand
        .stage1
        .blocks
        .iter()
        .map(|block| block.bytes as usize + pages * spare_bytes)
        .sum();
    if bytes.len() != expected {
        return Err(OpenError::at(
            Stage::BootRom,
            format!(
                "the kept internal flash is {} bytes; this firmware's stage-1 blocks need {expected}",
                bytes.len()
            ),
        ));
    }
    let mut at = 0;
    for block in &nand.stage1.blocks {
        let data_bytes = block.bytes as usize;
        let first = (block.native_offset / page) as u32;
        medium
            .erase(first)
            .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        for index in 0..pages {
            let data = &bytes[at + index * page as usize..at + (index + 1) * page as usize];
            let spare_at = at + data_bytes + index * spare_bytes;
            medium
                .program(
                    first + index as u32,
                    data,
                    &bytes[spare_at..spare_at + spare_bytes],
                )
                .map_err(|error| OpenError::at(Stage::BootRom, error))?;
        }
        at += data_bytes + pages * spare_bytes;
    }
    session
        .retain_nand("nand", medium)
        .map_err(|error| OpenError::at(Stage::BootRom, error))
}
