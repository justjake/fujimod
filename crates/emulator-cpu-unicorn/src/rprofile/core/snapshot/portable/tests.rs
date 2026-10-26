use super::*;
use crate::rprofile::{
    core::tests::{get, processor_with_options, resume, run, set, words},
    tcm,
};

fn options() -> Options {
    Options {
        cp14_identity: 0x1234,
        tcm: tcm::Options {
            btcm: tcm::BankOptions {
                bytes: 0x1000,
                reset: 0x70000001,
            },
            atcm: tcm::BankOptions {
                bytes: 0x1000,
                reset: 0x70001001,
            },
        },
    }
}
fn processor(options: Options) -> Core {
    let cpu = processor_with_options(&Memory::default(), options);
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xe2800001, 0xeafffffd]);
    set(&cpu, arm::CPSR, 0xd3);
    set(&cpu, arm::PC, 0x1000);
    cpu
}
fn cp(crn: u32, crm: u32, opc2: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn,
        crm,
        opc2,
        secure: 0,
        wide: false,
    }
}
fn domain(cpu: &Core, bytes: &[u8]) {
    let mut monitors = cpu.execution.memory.monitors.lock().unwrap();
    let rebound = monitors.import_checkpoint(bytes).unwrap();
    monitors.restore_checkpoint(&rebound).unwrap();
}

#[test]
fn fresh_processor_restores_architecture_tcm_pmu_clock_and_accounting_continuation() {
    let (bytes, monitors, at_capture, expected) = {
        let cpu = processor(options());
        cpu.rprofile_identity(Some(0x5678)).unwrap();
        cpu.write_memory(0x70000023, &[0xa5, 0x5a]).unwrap();
        cpu.write_memory(0x70001ff0, &[1, 2, 3, 4]).unwrap();
        cpu.set_guest_clock(Clock {
            now: 100,
            phase: 7,
            ratio: fujimod_emulator_cpu::timeline::Ratio::new(24, 1).unwrap(),
        })
        .unwrap();
        cpu.coprocessor(cp(9, 12, 0), Some(1)).unwrap();
        cpu.coprocessor(cp(9, 12, 1), Some(0x80000000)).unwrap();
        run(&cpu, 0x1000, 35).unwrap();
        let saved = cpu.snapshot(None).unwrap();
        let bytes = saved.export().unwrap();
        let monitors = cpu
            .execution
            .memory
            .monitors
            .lock()
            .unwrap()
            .export_checkpoint();
        let at_capture = cpu.fingerprint().unwrap();
        cpu.write_memory(0x70000023, &[0, 0]).unwrap();
        cpu.rprofile_identity(Some(0xffff)).unwrap();
        cpu.set_interrupt(Interrupt::Irq, true).unwrap();
        let changed = cpu.fingerprint().unwrap();
        assert_eq!(saved.export().unwrap(), bytes);
        assert_eq!(cpu.fingerprint().unwrap(), changed);
        cpu.restore(saved.as_ref()).unwrap();
        assert_eq!(cpu.rprofile_identity(None).unwrap(), 0x5678);
        resume(&cpu, 2113).unwrap();
        (bytes, monitors, at_capture, cpu.fingerprint().unwrap())
    };
    let target = processor(options());
    let before = target.fingerprint().unwrap();
    let imported = target.import_snapshot(&bytes).unwrap();
    assert_eq!(target.fingerprint().unwrap(), before);
    target.restore(imported.as_ref()).unwrap();
    domain(&target, &monitors);
    assert_eq!(target.fingerprint().unwrap(), at_capture);
    assert_eq!(
        target
            .snapshot(Some(imported.as_ref()))
            .unwrap()
            .export()
            .unwrap(),
        bytes
    );
    resume(&target, 2113).unwrap();
    assert_eq!(target.fingerprint().unwrap(), expected);
}

#[test]
fn invalid_state_is_refused_before_cpu_private_memory_or_inputs_change() {
    let cpu = processor(options());
    let original = cpu.snapshot(None).unwrap().export().unwrap();
    let before = cpu.fingerprint().unwrap();
    for corrupt in 0..10 {
        let mut saved: State = decode(&original).unwrap();
        match corrupt {
            0 => saved.format.push('x'),
            1 => saved.tcm[0].len += 1,
            2 => saved.tcm[1].register = saved.tcm[0].register,
            3 => saved.clock = Some((1, 24, (24, 1))),
            4 => saved.policy = "unknown".into(),
            5 => {
                saved.ring.pop();
            }
            6 => saved.hint = Some(255),
            7 => saved.exception = Some((255, 0)),
            8 => saved.reservation.clear(),
            _ => saved.context.clear(),
        }
        assert!(cpu.import_snapshot(&encode(&saved).unwrap()).is_err());
        assert_eq!(cpu.fingerprint().unwrap(), before);
    }
    let other = processor(Options::default());
    let before = other.fingerprint().unwrap();
    assert!(other.import_snapshot(&original).is_err());
    assert_eq!(other.fingerprint().unwrap(), before);
}

#[test]
fn decode_binds_external_host_addresses_only_after_the_receiving_mapping_matches() {
    let source = processor(Options::default());
    source.map_zeroed(0x3000, 0x1000, 3).unwrap();
    set(&source, arm::CORE[0], 42);
    let bytes = source.snapshot(None).unwrap().export().unwrap();
    let target = processor(Options::default());
    let saved = target.import_snapshot(&bytes).unwrap();
    let before = target.fingerprint().unwrap();
    assert!(target.restore(saved.as_ref()).is_err());
    assert_eq!(target.fingerprint().unwrap(), before);
    target.map_zeroed(0x3000, 0x1000, 3).unwrap();
    target.restore(saved.as_ref()).unwrap();
    assert_eq!(get(&target, arm::CORE[0]), 42);
}

#[test]
fn portable_mapping_identity_is_independent_of_host_allocation_adjacency() {
    use fujimod_emulator_cpu::memory::Backing;
    struct Processor {
        cpu: Core,
        _backing: Vec<Backing>,
    }
    fn mapped(split: bool) -> Processor {
        let cpu = processor(Options::default());
        let backing = if split {
            vec![
                Backing::aligned_zeros(0x1000, 4096).unwrap(),
                Backing::aligned_zeros(0x1000, 4096).unwrap(),
            ]
        } else {
            vec![Backing::aligned_zeros(0x2000, 4096).unwrap()]
        };
        for i in 0..2 {
            let pointer = if split {
                backing[i].as_ptr()
            } else {
                unsafe { backing[0].as_ptr().add(i * 0x1000) }
            };
            unsafe {
                cpu.map_physical(
                    0x8000 + i as u64 * 0x1000,
                    0x8000 + i as u64 * 0x1000,
                    0x1000,
                    3,
                    NonNull::new(pointer).unwrap(),
                )
            }
            .unwrap();
        }
        Processor {
            cpu,
            _backing: backing,
        }
    }
    let source = mapped(false);
    set(&source.cpu, arm::CORE[0], 37);
    let bytes = source.cpu.snapshot(None).unwrap().export().unwrap();
    let target = mapped(true);
    let imported = target.cpu.import_snapshot(&bytes).unwrap();
    target.cpu.restore(imported.as_ref()).unwrap();
    assert_eq!(
        source.cpu.fingerprint().unwrap(),
        target.cpu.fingerprint().unwrap()
    );
}

#[test]
fn whole_domain_rebinding_preserves_exclusives_and_later_aba_writes_invalidate_them() {
    struct Processor {
        cpu: Core,
        _ram: fujimod_emulator_cpu::memory::Backing,
    }
    fn exclusive() -> Processor {
        let cpu = processor(Options::default());
        let ram = fujimod_emulator_cpu::memory::Backing::aligned_zeros(0x1000, 4096).unwrap();
        unsafe {
            cpu.map_physical(
                0x2000,
                0x2000,
                0x1000,
                3,
                NonNull::new(ram.as_ptr()).unwrap(),
            )
        }
        .unwrap();
        words(&cpu, 0x1000, &[0xe1910f9f, 0xe1812f93, 0xeafffffe]); // ldrex r0,[r1]; strex r2,r3,[r1]; b .
        words(&cpu, 0x2000, &[37]);
        set(&cpu, arm::CORE[1], 0x2000);
        set(&cpu, arm::CORE[3], 44);
        Processor { cpu, _ram: ram }
    }
    let (bytes, monitors) = {
        let source = exclusive();
        run(&source.cpu, 0x1000, 1).unwrap();
        let bytes = source.cpu.snapshot(None).unwrap().export().unwrap();
        let monitors = source
            .cpu
            .execution
            .memory
            .monitors
            .lock()
            .unwrap()
            .export_checkpoint();
        (bytes, monitors)
    };
    for aba in [false, true] {
        let processor = exclusive();
        let target = &processor.cpu;
        let saved = target.import_snapshot(&bytes).unwrap();
        target.restore(saved.as_ref()).unwrap();
        domain(&target, &monitors);
        if aba {
            words(&target, 0x2000, &[37]);
        }
        resume(&target, 1).unwrap();
        assert_eq!(get(&target, arm::CORE[2]), u32::from(aba));
        let mut bytes = [0; 4];
        target.read_memory(0x2000, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes), if aba { 37 } else { 44 });
    }
}
