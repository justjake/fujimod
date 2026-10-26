//! Test support for the EXR II ISP live-view models: a recording memory host and an
//! allocation counter. Test-only: it installs a counting global allocator.
use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use fujimod_emulator_devices::{Cx, Device, Host, Record};

// Measure successful device calls on this test thread; allocations made by
// unrelated tests and the harness outside the measurement are excluded.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {pub static ALLOCATIONS:Cell<Option<usize>>=const {Cell::new(None)};}
struct CountingAllocator;
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| {
            if let Some(v) = n.get() {
                n.set(Some(v + 1));
            }
        });
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| {
            if let Some(v) = n.get() {
                n.set(Some(v + 1));
            }
        });
        unsafe { System.realloc(pointer, layout, bytes) }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Dma(u64, usize),
    Irq(usize, u32),
}
pub struct Memory {
    pub now: u64,
    pub bytes: Vec<u8>,
    pub effects: Vec<Effect>,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            now: 0,
            bytes: vec![0; 1 << 20],
            effects: Vec::new(),
        }
    }
}
impl Host for Memory {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("models use exact next_deadline")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("EXR II uses per-core IRQ inputs")
    }
    fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
        self.effects.push(Effect::Irq(cpu, line));
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let address = (address & 0x3fffffff) as usize;
        out.copy_from_slice(
            self.bytes
                .get(address..address + out.len())
                .ok_or("unmapped test RAM")?,
        );
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let physical = (address & 0x3fffffff) as usize;
        self.bytes
            .get_mut(physical..physical + bytes.len())
            .ok_or("unmapped test RAM")?
            .copy_from_slice(bytes);
        self.effects.push(Effect::Dma(address, bytes.len()));
        Ok(())
    }
    fn record(&mut self, _: Record) {
        panic!("normal device path must not build records")
    }
}
pub fn write(d: &mut dyn Device, m: &mut Memory, address: u64, value: u64) {
    let mut cx = Cx::new(m, 0, 0, 0);
    d.write(&mut cx, address, 4, value);
    assert_eq!(cx.refusal(), None);
}
pub fn reject(d: &mut dyn Device, m: &mut Memory, address: u64, value: u64) {
    let mut cx = Cx::new(m, 1, 0xdeadbeef, 0);
    d.write(&mut cx, address, 4, value);
    assert!(cx.refusal().is_some());
}
pub fn read(d: &mut dyn Device, m: &mut Memory, address: u64) -> u64 {
    let mut cx = Cx::new(m, 1, 0x12345678, 0);
    let value = d.read(&mut cx, address, 4);
    assert_eq!(cx.refusal(), None);
    value
}
pub fn advance(d: &mut dyn Device, m: &mut Memory, tick: u64) {
    m.now = tick;
    let mut cx = Cx::new(m, 0, 0, 0);
    d.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
pub fn routes(line: u32) -> IrqRoutes {
    let r = IrqRoutes::default();
    r.set(line, Some(0)).unwrap();
    r
}
