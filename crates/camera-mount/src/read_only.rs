//! A device that refuses every change to the camera's SD card.

use crate::{display, DirectoryEntry, Device, Result};

/// Passes reads to `D` and refuses writes, creation, deletion and truncation
/// with `EROFS`, so neither a user nor the host's own metadata writes can
/// change the card.
pub struct ReadOnly<D>(pub D);

impl<D: Device> Device for ReadOnly<D> {
    fn shutdown(&mut self) -> Result<()> {
        self.0.shutdown()
    }
    fn unload_helper(&mut self) -> Result<()> {
        self.0.unload_helper()
    }
    fn display_outputs(&self) -> Vec<u32> {
        self.0.display_outputs()
    }
    fn display_images(&mut self, output: u32) -> Result<Vec<display::Image>> {
        self.0.display_images(output)
    }
    fn has_card(&self) -> bool {
        self.0.has_card()
    }
    fn list(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        self.0.list(path)
    }
    fn read(&mut self, path: &str, offset: u32, size: u32) -> Result<Vec<u8>> {
        self.0.read(path, offset, size)
    }
    fn write(&mut self, _: &str, _: u32, _: &[u8]) -> Result<()> {
        Err(libc::EROFS)
    }
    fn create(&mut self, _: &str) -> Result<()> {
        Err(libc::EROFS)
    }
    fn delete(&mut self, _: &str) -> Result<()> {
        Err(libc::EROFS)
    }
    fn shrink(&mut self, _: &str, _: u32) -> Result<()> {
        Err(libc::EROFS)
    }
    fn memory(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        self.0.memory(address, size)
    }
    fn flash_size(&self) -> Option<u64> {
        self.0.flash_size()
    }
    fn flash(&mut self, offset: u64, size: u32) -> Result<Vec<u8>> {
        self.0.flash(offset, size)
    }
    fn synchronize(&mut self) -> Result<()> {
        self.0.synchronize()
    }
}
