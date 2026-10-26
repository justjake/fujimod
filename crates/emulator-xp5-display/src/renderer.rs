//! XP5 renderer DMA high-address register. The DMP engine is shared.
use fujimod_emulator_devices::{Cx, Device};

pub struct AddressHigh;
impl Device for AddressHigh {
    fn name(&self) -> &'static str {
        "dmp-pixmap-high-zero"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90e73c);
        if o != 0 || n != 4 {
            cx.unmodeled();
        }
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90e73c);
        if o != 0 || n != 4 {
            cx.unmodeled();
        } else if v != 0 {
            cx.fail("DMP high-address pixmap DMA is unresolved");
        }
    }
}
