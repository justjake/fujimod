use super::tests::{get, processor_with_options, resume, run, set, words};
use super::*;
use crate::rprofile::tcm::{BankOptions, Options as TcmOptions};
fn region(bank: u32) -> arm::Coprocessor {
    arm::Coprocessor {
        cp: 15,
        opc1: 0,
        crn: 9,
        crm: 1,
        opc2: bank,
        secure: 0,
        wide: false,
    }
}
fn options(bytes: usize, reset: u32) -> Options {
    Options {
        tcm: TcmOptions {
            atcm: BankOptions { bytes, reset },
            ..TcmOptions::default()
        },
        ..Options::default()
    }
}
fn read(cpu: &Core, address: u64) -> u32 {
    let mut bytes = [0; 4];
    cpu.read_memory(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}
#[test]
fn moving_and_disabling_tcm_preserves_private_bytes_and_restores_real_underlay_permissions() {
    let backing = Backing::aligned_zeros(0x8000, 4096).unwrap();
    unsafe {
        std::ptr::write_bytes(backing.as_ptr().add(0x4000), 0xaa, 0x2000);
        std::ptr::write_bytes(backing.as_ptr().add(0x6000), 0x55, 0x2000);
    }
    let cpu = processor_with_options(&Memory::default(), options(0x2000, 0x8001));
    assert_eq!(cpu.coprocessor(region(1), None).unwrap(), 0x8011);
    unsafe {
        cpu.map_backing(0x4000, 0x8000, 5, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    assert_eq!(read(&cpu, 0x8000), 0);
    words(&cpu, 0x8000, &[0x12345678]);
    cpu.coprocessor(region(1), Some(0xa7ff)).unwrap();
    assert_eq!(cpu.coprocessor(region(1), None).unwrap(), 0xa011);
    assert_eq!(
        (read(&cpu, 0x8000), read(&cpu, 0xa000)),
        (0xaaaaaaaa, 0x12345678)
    );
    cpu.coprocessor(region(1), Some(0xa010)).unwrap();
    assert_eq!(read(&cpu, 0xa000), 0x55555555);
    words(&cpu, 0x1000, &[0xe5812000]); // str r2,[r1] into restored read-only RAM
    set(&cpu, arm::CORE[1], 0x8000);
    set(&cpu, arm::CORE[2], 42);
    assert!(matches!(
        run(&cpu, 0x1000, 1),
        Err(ExecutionError::Memory {
            fault: Fault::WriteProtected,
            ..
        })
    ));
    assert_eq!(read(&cpu, 0x8000), 0xaaaaaaaa);
    cpu.coprocessor(region(1), Some(0x8001)).unwrap();
    assert_eq!(read(&cpu, 0x8000), 0x12345678);
    assert!(cpu.unmap(0x8000, 0x1000).is_err());
    assert!(cpu.protect(0x8000, 0x1000, 5).is_err());
}

#[test]
fn external_replacement_under_tcm_preserves_private_bytes_and_reservations() {
    let first = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let second = Backing::aligned_zeros(0x1000, 4096).unwrap();
    unsafe {
        std::ptr::write_unaligned(first.as_ptr().cast::<u32>(), 0xaaaa);
        std::ptr::write_unaligned(second.as_ptr().cast::<u32>(), 0xbbbb);
    }
    let cpu = processor_with_options(&Memory::default(), options(0x1000, 0x1001));
    unsafe {
        cpu.map_physical(0x1000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
        cpu.map_physical(0x5000, 0, 0x1000, 7, NonNull::new(first.as_ptr()).unwrap())
            .unwrap();
    }
    cpu.map_zeroed(0x9000, 0x1000, 7).unwrap();
    words(&cpu, 0x1000, &[0xcccc]);
    words(&cpu, 0x9000, &[0xe1910f9f, 0xe1813f92]);
    set(&cpu, arm::CORE[1], 0x1000);
    set(&cpu, arm::CORE[2], 0xdddd);
    run(&cpu, 0x9000, 1).unwrap();
    unsafe {
        cpu.replace_physical(
            0x1000,
            0x8000,
            0x1000,
            5,
            NonNull::new(second.as_ptr()).unwrap(),
        )
        .unwrap();
    }
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0);
    assert_eq!(read(&cpu, 0x1000), 0xdddd);
    assert_eq!(read(&cpu, 0x5000), 0xaaaa);
    cpu.coprocessor(region(1), Some(0x1000)).unwrap();
    assert_eq!(read(&cpu, 0x1000), 0xbbbb);
    words(&cpu, 0x9000, &[0xe5812000]);
    assert!(matches!(
        run(&cpu, 0x9000, 1),
        Err(ExecutionError::Memory {
            fault: Fault::WriteProtected,
            ..
        })
    ));
    cpu.coprocessor(region(1), Some(0x1001)).unwrap();
    assert_eq!(read(&cpu, 0x1000), 0xdddd);
}
#[test]
fn remap_respects_every_entered_instruction_limit_before_and_after_the_move() {
    for cap in 1..=3 {
        let cpu = processor_with_options(&Memory::default(), options(0x1000, 0x8001));
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        words(&cpu, 0x8000, &[0xe2822009]);
        words(&cpu, 0x1000, &[0xee090f31, 0xe12fff11]);
        set(&cpu, arm::CORE[0], 0xa001);
        set(&cpu, arm::CORE[1], 0xa000);
        run(&cpu, 0x1000, cap).unwrap();
        assert_eq!(cpu.instruction_progress(), Some(cap as u64));
        assert_eq!(get(&cpu, arm::PC), [0x1004, 0xa000, 0xa004][cap - 1]);
        if cap < 3 {
            resume(&cpu, 3 - cap).unwrap();
        }
        assert_eq!(get(&cpu, arm::CORE[2]), 9);
    }
}

#[test]
fn guest_tcm_remap_continues_at_the_next_instruction_without_extra_entries() {
    let cpu = processor_with_options(&Memory::default(), options(0x1000, 0x8001));
    cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
    cpu.map_zeroed(0x8000, 0x1000, 7).unwrap();
    words(&cpu, 0x8000, &[0xe2822009]);
    words(&cpu, 0x1000, &[0xee090f31, 0xe12fff11, 0xee193f31]); // mcr atcm,r0; bx r1; mrc atcm,r3
    set(&cpu, arm::CORE[0], 0xa001);
    set(&cpu, arm::CORE[1], 0xa000);
    run(&cpu, 0x1000, 3).unwrap();
    assert_eq!(
        (
            cpu.instruction_progress(),
            get(&cpu, arm::PC),
            get(&cpu, arm::CORE[2])
        ),
        (Some(3), 0xa004, 9)
    );
    assert_eq!(read(&cpu, 0x8000), 0);
    run(&cpu, 0x1008, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0xa00d);
    // Reads and normalized no-op writes leave a reservation intact.
    words(&cpu, 0x1000, &[0xe1910f9f, 0xe1813f92]);
    set(&cpu, arm::CORE[1], 0xa800);
    set(&cpu, arm::CORE[2], 17);
    run(&cpu, 0x1000, 1).unwrap();
    cpu.coprocessor(region(1), None).unwrap();
    cpu.coprocessor(region(1), Some(0xa7ff)).unwrap();
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 0);
    run(&cpu, 0x1000, 1).unwrap();
    cpu.coprocessor(region(1), Some(0xc001)).unwrap();
    set(&cpu, arm::CORE[1], 0xc800);
    resume(&cpu, 1).unwrap();
    assert_eq!(get(&cpu, arm::CORE[3]), 1);
}
#[test]
fn tcm_interfaces_are_private_and_invalid_register_transactions_leave_maps_intact() {
    let memory = Memory::default();
    let a = processor_with_options(&memory, options(0x1000, 0x8001));
    let b = processor_with_options(&memory, options(0x1000, 0x8001));
    words(&a, 0x8000, &[33]);
    assert_eq!(read(&b, 0x8000), 0);
    assert!(
        a.coprocessor(region(0), Some(1))
            .unwrap_err()
            .contains("absent")
    );
    assert_eq!(a.coprocessor(region(0), None).unwrap(), 0);
    a.map_zeroed(0x1000, 0x1000, 7).unwrap();
    words(&a, 0x1000, &[0xee090f31]);
    set(&a, arm::CORE[0], 0xa001);
    set(&a, arm::CPSR, 0x10);
    assert!(
        run(&a, 0x1000, 1)
            .unwrap_err()
            .to_string()
            .contains("privileged")
    );
    assert_eq!(a.coprocessor(region(1), None).unwrap(), 0x800d);
    assert_eq!(read(&a, 0x8000), 33);
    unsafe fn no_read(_: u64, _: u32, _: *mut c_void) -> u64 {
        panic!("TCM validation drove a device");
    }
    unsafe fn no_write(_: u64, _: u32, _: u64, _: *mut c_void) {
        panic!("TCM validation drove a device");
    }
    unsafe {
        a.map_mmio(
            0xa000,
            0x1000,
            Mmio {
                read: no_read,
                write: no_write,
                data: std::ptr::null_mut(),
            },
        )
        .unwrap();
    }
    assert!(
        a.coprocessor(region(1), Some(0xa001))
            .unwrap_err()
            .contains("MMIO")
    );
    assert_eq!(a.coprocessor(region(1), None).unwrap(), 0x800d);
    assert_eq!(read(&a, 0x8000), 33);
}

#[test]
fn both_tcm_interfaces_enforce_disjoint_registers_and_independent_bytes() {
    let cpu = processor_with_options(
        &Memory::default(),
        Options {
            tcm: TcmOptions {
                btcm: BankOptions {
                    bytes: 0x1000,
                    reset: 0x6001,
                },
                atcm: BankOptions {
                    bytes: 0x1000,
                    reset: 0x8001,
                },
            },
            ..Options::default()
        },
    );
    words(&cpu, 0x6000, &[11]);
    words(&cpu, 0x8000, &[22]);
    assert!(
        cpu.coprocessor(region(1), Some(0x6001))
            .unwrap_err()
            .contains("overlap")
    );
    assert_eq!((read(&cpu, 0x6000), read(&cpu, 0x8000)), (11, 22));
    assert_eq!(cpu.coprocessor(region(1), None).unwrap(), 0x800d);
    cpu.coprocessor(region(1), Some(0xa001)).unwrap();
    assert_eq!((read(&cpu, 0x6000), read(&cpu, 0xa000)), (11, 22));
}
#[test]
fn disabling_the_executing_tcm_uses_the_restored_instruction_bytes() {
    let backing = Backing::aligned_zeros(0x1000, 4096).unwrap();
    let code = [0xe320f000u32, 0xe2822009];
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr().cast::<u8>(), backing.as_ptr(), 8);
    }
    let cpu = processor_with_options(&Memory::default(), options(0x1000, 0x8001));
    unsafe {
        cpu.map_backing(0x8000, 0x1000, 5, NonNull::new(backing.as_ptr()).unwrap())
            .unwrap();
    }
    words(&cpu, 0x8000, &[0xee090f31, 0xe2822001]);
    set(&cpu, arm::CORE[0], 0x8000);
    run(&cpu, 0x8000, 2).unwrap();
    assert_eq!(
        (cpu.instruction_progress(), get(&cpu, arm::CORE[2])),
        (Some(2), 9)
    );
}
