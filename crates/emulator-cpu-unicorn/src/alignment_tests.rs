//! Data alignment while the Cortex-A7 MMU is off.
//!
//! With the VMSA stage 1 MMU off every data access is Strongly-ordered
//! (ARMv7-A ARM B3.2.1), so an unaligned word, halfword or doubleword access
//! takes an alignment fault even with SCTLR.A clear. Camera run 2026-11-10: an
//! X-T3 SD card whose FFINIT-stage reader had unaligned `ldr`/`ldrh` loads
//! showed no LED and hung, where the emulator had loaded the file; the same
//! reader built with `-mno-unaligned-access` boots in about 10 s.
//! The backend reports the alignment fault as a data abort and performs no
//! part of the access, so a board refuses it instead of loading the data.
use crate::{Core, test_support};
use fujimod_emulator_cpu::{Cpu, Hook, Model, Register, Run, arm};
use std::{cell::RefCell, ffi::c_void};

const CODE: u64 = 0x1000;
const ACCESSES: [(u32, &str); 5] = [
    (0xe590_2000, "ldr r2, [r0]"),
    (0xe1d0_20b0, "ldrh r2, [r0]"),
    (0xe580_2000, "str r2, [r0]"),
    (0xe1c0_20b0, "strh r2, [r0]"),
    (0xe1c0_20d0, "ldrd r2, r3, [r0]"),
];
const LDRB: u32 = 0xe5d0_2000;

fn cp(crn: u32, crm: u32, opc1: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor { cp: 15, crn, crm, opc1, opc2, secure: 1, wide: false }
}
fn set(cpu: &Core, register: Register, value: u32) {
    cpu.write_register(register, &value.to_le_bytes()).unwrap();
}
fn reg(cpu: &Core, register: Register) -> u32 {
    let mut out = [0; 4];
    assert_eq!(cpu.read_register(register, &mut out).unwrap(), 4);
    u32::from_le_bytes(out)
}

/// Secure SVC with `instruction` at CODE and r0 = `address`.
fn core(instruction: u32, address: u32) -> Core {
    let cpu = test_support::library().open(Model::CortexA7, 1 << 20).unwrap();
    cpu.map_zeroed(0, 0x40_0000, 7).unwrap();
    cpu.write_memory(CODE, &instruction.to_le_bytes()).unwrap();
    cpu.write_memory(0x2000, &0x4433_2211u32.to_le_bytes()).unwrap();
    cpu.write_memory(0x2004, &0x8877_6655u32.to_le_bytes()).unwrap();
    // Secure SVC: SCR.NS=0 selects the Secure SCTLR, TTBR0 and DACR below.
    cpu.coprocessor(cp(1, 1, 0, 0), Some(0)).unwrap();
    set(&cpu, arm::CPSR, 0xd3);
    assert_eq!(cpu.coprocessor(cp(1, 1, 0, 0), None).unwrap(), 0);
    set(&cpu, arm::CORE[0], address);
    cpu
}

unsafe fn taken(exception: arm::Exception, data: *mut c_void) {
    unsafe { &*data.cast::<RefCell<Vec<arm::Exception>>>() }.borrow_mut().push(exception);
}

/// Execute the one instruction at CODE; returns the exceptions it took.
fn step(cpu: &Core) -> Vec<arm::Exception> {
    let exceptions = Box::new(RefCell::new(Vec::new()));
    let data = (&*exceptions as *const RefCell<Vec<arm::Exception>>).cast_mut().cast();
    let hook = unsafe { cpu.install_hook(Hook::Exception { callback: taken, data }) }.unwrap();
    cpu.run(Run { begin: CODE, until: Some(CODE + 4), instructions: Some(1), watchdog_us: 1_000_000 })
        .unwrap();
    cpu.remove_hook(hook).unwrap();
    exceptions.into_inner()
}

/// Identity-map the first 4 MiB with Normal sections and enable the MMU.
fn translation_on(cpu: &Core) {
    for index in 0..4u32 {
        let section = index << 20 | 0x0c0e; // AP=0b11, C=1, B=1: Normal.
        cpu.write_memory(0x4000 + u64::from(index) * 4, &section.to_le_bytes()).unwrap();
    }
    cpu.coprocessor(cp(2, 0, 0, 0), Some(0x4000)).unwrap(); // TTBR0.
    cpu.coprocessor(cp(3, 0, 0, 0), Some(0xffff_ffff)).unwrap(); // DACR: manager.
    let sctlr = cpu.coprocessor(cp(1, 0, 0, 0), None).unwrap();
    cpu.coprocessor(cp(1, 0, 0, 0), Some(sctlr | 1)).unwrap();
}

#[test]
fn unaligned_data_access_with_the_mmu_off_takes_an_alignment_fault() {
    for (instruction, name) in ACCESSES {
        let cpu = core(instruction, 0x2001);
        assert_eq!(cpu.coprocessor(cp(1, 0, 0, 0), None).unwrap() & 3, 0, "MMU and SCTLR.A off");
        let taken = step(&cpu);
        assert!(!taken.is_empty() && taken.iter().all(|e| *e == arm::Exception::DataAbort),
            "{name} with the MMU off: {taken:?}");
        // The access is refused: no register or memory changes, no progress.
        assert_eq!(reg(&cpu, arm::PC), CODE as u32, "{name}");
        assert_eq!((reg(&cpu, arm::CORE[2]), reg(&cpu, arm::CORE[3])), (0, 0), "{name}");
        let mut memory = [0; 8];
        cpu.read_memory(0x2000, &mut memory).unwrap();
        assert_eq!(memory, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88], "{name}");
    }
}

#[test]
fn aligned_and_byte_accesses_with_the_mmu_off_complete() {
    for (instruction, name) in ACCESSES {
        let cpu = core(instruction, 0x2000);
        assert_eq!(step(&cpu), [], "{name} aligned");
        assert_eq!(reg(&cpu, arm::PC), CODE as u32 + 4, "{name}");
    }
    let cpu = core(LDRB, 0x2003);
    assert_eq!(step(&cpu), []);
    assert_eq!(reg(&cpu, arm::CORE[2]), 0x44);
}

#[test]
fn unaligned_access_to_normal_memory_with_the_mmu_on_completes() {
    let cpu = core(ACCESSES[0].0, 0x2001);
    translation_on(&cpu);
    assert_eq!(step(&cpu), [], "ldr on Normal memory with SCTLR.A clear");
    assert_eq!(reg(&cpu, arm::CORE[2]), 0x5544_3322);
}
