//! A processor executes a device page that offers execution from one copy
//! read through the device, and reads it again once the device's epoch moves.
use super::*;
use fujimod_emulator_cpu::{Model, arm};
use fujimod_emulator_cpu_unicorn::Library;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const FLASH: u64 = 0x8000000;
const ADD_ONE: u32 = 0xe286_6001; // add r6, r6, #1
const ADD_TWO: u32 = 0xe286_6002; // add r6, r6, #2
const RETURN: u32 = 0xe12f_ff1e; // bx lr

fn set(cpu: &dyn Cpu, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}

fn get(cpu: &dyn Cpu, register: Register) -> u32 {
    let mut bytes = [0; 4];
    cpu.read_register(register, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

struct Flash {
    words: Arc<std::sync::Mutex<Vec<u32>>>,
    epoch: Arc<AtomicU64>,
    reads: Arc<AtomicU64>,
    executable: bool,
}
impl fujimod_emulator_devices::capability::ExecuteInPlace for Flash {
    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
}
impl fujimod_emulator_devices::Device for Flash {
    fn name(&self) -> &'static str {
        "flash"
    }
    fn read(&mut self, _: &mut fujimod_emulator_devices::Cx<'_>, address: u64, _: u8) -> u64 {
        self.reads.fetch_add(1, Ordering::SeqCst);
        u64::from(self.words.lock().unwrap()[(address / 4) as usize % 1024])
    }
    fn write(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, _: u64, _: u8, _: u64) {
        cx.fail("flash is read-only");
    }
    fn save(&self) -> Value {
        json!({})
    }
    fn execute_in_place(
        &self,
    ) -> Option<&(dyn fujimod_emulator_devices::capability::ExecuteInPlace + 'static)> {
        self.executable.then_some(self as _)
    }
}

struct Fixture {
    board: Board,
    words: Arc<std::sync::Mutex<Vec<u32>>>,
    epoch: Arc<AtomicU64>,
    reads: Arc<AtomicU64>,
}

fn fixture(executable: bool) -> Fixture {
    let library = Library::linked().unwrap();
    let raw = library.open(Model::CortexR5F, 4 << 20).unwrap();
    let cpu: Box<dyn Cpu> = Box::new(
        fujimod_emulator_cpu_unicorn::rprofile::core::Core::new(
            raw,
            fujimod_emulator_cpu_unicorn::rprofile::core::Options::default(),
            fujimod_emulator_cpu_unicorn::rprofile::core::Wiring {
                memory: Default::default(),
            },
        )
        .unwrap(),
    );
    cpu.map_zeroed(0x10000, 0x1000, 7).unwrap();
    // blx r5; b .
    cpu.write_memory(0x10000, &[0xe12f_ff35u32, 0xeaff_fffe].map(u32::to_le_bytes).concat())
        .unwrap();
    let mut words = vec![0u32; 1024];
    words[0] = ADD_ONE;
    words[1] = RETURN;
    let words = Arc::new(std::sync::Mutex::new(words));
    let epoch = Arc::new(AtomicU64::new(0));
    let reads = Arc::new(AtomicU64::new(0));
    let mut bus = Bus::new(bus::Mode::Strict);
    bus.register(
        FLASH,
        0x1000,
        Box::new(Flash { words: words.clone(), epoch: epoch.clone(), reads: reads.clone(), executable }),
    )
    .unwrap();
    let mut board = Board::new(
        vec![cpu],
        bus,
        Ratio::new(1, 1).unwrap(),
        None,
        vec![true],
        memory::Memory::default(),
    )
    .unwrap();
    board.map_mmio(0, FLASH, 0x1000, FLASH).unwrap();
    board.set_execution_policy(ExecutionPolicy::Exact).unwrap();
    Fixture { board, words, epoch, reads }
}

fn call(board: &mut Board) -> Result<(), String> {
    let cpu = board.cpu(0).unwrap();
    set(cpu, arm::CORE[5], FLASH as u32);
    set(cpu, arm::CPSR, 0xd3);
    set(cpu, Register::Pc, 0x10000);
    board.run_for_instructions(100, 100)
}

#[test]
fn a_fetched_page_is_read_once_and_executes() {
    let mut f = fixture(true);
    call(&mut f.board).unwrap();
    let cpu = f.board.cpu(0).unwrap();
    assert_eq!(get(cpu, arm::CORE[6]), 1);
    assert_eq!(get(cpu, Register::Pc), 0x10004);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1024);
    call(&mut f.board).unwrap();
    assert_eq!(get(f.board.cpu(0).unwrap(), arm::CORE[6]), 2);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1024, "the copy holds while the epoch stands");
}

#[test]
fn a_moved_epoch_returns_the_page_to_its_device() {
    let mut f = fixture(true);
    call(&mut f.board).unwrap();
    f.words.lock().unwrap()[0] = ADD_TWO;
    f.epoch.fetch_add(1, Ordering::SeqCst);
    call(&mut f.board).unwrap();
    assert_eq!(get(f.board.cpu(0).unwrap(), arm::CORE[6]), 3);
    assert_eq!(f.reads.load(Ordering::SeqCst), 2048);
}

#[test]
fn a_device_that_offers_no_execution_keeps_the_fault() {
    let mut f = fixture(false);
    let error = call(&mut f.board).unwrap_err();
    assert!(error.to_lowercase().contains("fetch"), "{error}");
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
}
