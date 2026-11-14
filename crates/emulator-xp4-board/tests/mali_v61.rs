//! The XP4 Mali-V61 style video unit (X-T3 5.11: page 0xffe00000, interrupt
//! 138): wiring admission, and one JOB answered through guest stores and
//! loads in the order the firmware's driver uses.
use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const CBAR: u32 = 0x2000_0000;
const UNIT: u32 = 0xffe0_0000;
const IRQ: u32 = 138;
const CODE: usize = 0x1000;
const RESULT: usize = 0x2000;
/// Two-level table: L0 at 0x4000, L1 at 0x5000; queue pages from 0x8000.
const L0: usize = 0x4000;
const L1: usize = 0x5000;
const PAGES: usize = 0x8000;
/// VPU address of queue page 0, as the X-T3 5.11 driver maps it.
const QUEUES: u32 = 0x1007_9000;

fn input() -> Value {
    json!({"ram_size": 65536, "cores": 1, "discovery": false, "cbar": CBAR,
        "initial_registers": [[["Pc", CODE]]]})
}

fn unit(base: u32, irq: u32, ticks: u64) -> Value {
    json!({"base": base, "irq": irq, "ticks": ticks, "queues": QUEUES})
}

fn session(value: Value) -> Result<Session, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Session::new(definition, None, vec![])
}

fn word(session: &Session, address: usize) -> u32 {
    let mut bytes = [0; 4];
    session.read(address, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

fn half(session: &Session, address: usize) -> u16 {
    let mut bytes = [0; 2];
    session.read(address, &mut bytes).unwrap();
    u16::from_le_bytes(bytes)
}

#[test]
fn the_unit_is_explicit_and_owns_its_page() {
    let absent = session(input()).unwrap();
    assert!(!absent.device_state_json().contains_key("xp4-mali-v61"));
    let mut selected = input();
    selected["mali_v61"] = unit(UNIT, IRQ, 8);
    let board = session(selected.clone()).unwrap();
    assert!(board.device_state_json().contains_key("xp4-mali-v61"));
    for address in [UNIT, UNIT + 0x220, UNIT + 0xffc] {
        let mut conflict = selected.clone();
        conflict["control_requests"] = json!([[address, 4]]);
        let error = session(conflict).err().unwrap();
        assert!(error.contains("Overlapping"), "{error}");
    }
    let mut extra = selected.clone();
    extra["mali_v61"]["extra"] = json!(true);
    assert!(serde_json::from_value::<Definition>(extra).is_err());
    for (base, irq, ticks) in [
        (UNIT + 4, IRQ, 8),
        (UNIT, 31, 8),
        (UNIT, 512, 8),
        (UNIT, IRQ, 0),
    ] {
        let mut invalid = input();
        invalid["mali_v61"] = unit(base, irq, ticks);
        let error = session(invalid).err().unwrap();
        assert!(error.contains("Invalid XP4 Mali-V61"), "{error}");
    }
}

#[test]
fn a_guest_job_is_answered_on_the_declared_interrupt_line() {
    let mut selected = input();
    selected["mali_v61"] = unit(UNIT, IRQ, 8);
    let board = session(selected).unwrap();

    // The session table maps the seven queue pages; then the driver's JOB
    // (code 0x3f1, 8 payload bytes) sits on host page 0, write position 3.
    let l0_entry = (L1 as u32 >> 10) | 3;
    board
        .write(L0 + ((QUEUES >> 22) as usize) * 4, &l0_entry.to_le_bytes())
        .unwrap();
    for page in 0..7u32 {
        let entry = ((PAGES as u32 + page * 0x1000) >> 10) | 3;
        let index = (((QUEUES + page * 0x1000) >> 12) & 0x3ff) as usize;
        board.write(L1 + index * 4, &entry.to_le_bytes()).unwrap();
    }
    let job: Vec<u8> = [0x3f1 | (8 << 16), 4, 0]
        .iter()
        .flat_map(|w: &u32| w.to_le_bytes())
        .collect();
    board.write(PAGES + 0x10, &job).unwrap();
    board.write(PAGES + 2, &3u16.to_le_bytes()).unwrap();

    // The driver's sequence: reset (+0x50) and read CORELSID (+0x10), set the
    // session's table (+0x204) and ring its doorbell (+0x220), poll IRQVE
    // (+0x18) bit 0. The program then waits for the test's go-ahead, writes
    // 0 to +0x21c (the handler's acknowledge) and reads IRQVE again.
    let program: [u32; 16] = [
        0xe580_1050, // STR r1, [r0, #0x50]
        0xe590_2010, // LDR r2, [r0, #0x10]
        0xe583_2000, // STR r2, [r3]
        0xe580_4204, // STR r4, [r0, #0x204]
        0xe580_1220, // STR r1, [r0, #0x220]
        0xe590_2018, // LDR r2, [r0, #0x18]
        0xe312_0001, // TST r2, #1
        0x0aff_fffc, // BEQ the load above
        0xe583_2004, // STR r2, [r3, #4]
        0xe593_600c, // LDR r6, [r3, #12]
        0xe356_0000, // CMP r6, #0
        0x0aff_fffc, // BEQ the load above
        0xe580_521c, // STR r5, [r0, #0x21c]
        0xe590_2018, // LDR r2, [r0, #0x18]
        0xe583_2008, // STR r2, [r3, #8]
        0xeaff_fffe, // B .
    ];
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    board.write(CODE, &bytes).unwrap();
    for (register, value) in [
        (0, UNIT),
        (1, 1),
        (3, RESULT as u32),
        (4, (L0 as u32 >> 10) | 3),
        (5, 0),
    ] {
        board
            .write_register(0, arm::CORE[register], value.into())
            .unwrap();
    }
    board.write(RESULT + 8, &0xdeadu32.to_le_bytes()).unwrap();
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());

    assert_eq!(word(&board, RESULT), 0xffff, "no session on any core");
    assert_eq!(word(&board, RESULT + 4) & 1, 1, "message pending");
    // The JOB was read (VPU page 1, +2) and answered with 2001 SWITCHED_IN
    // (VPU page 1, write position +0 and the ring at +0x10).
    assert_eq!(half(&board, PAGES + 0x1000 + 2), 3);
    assert_eq!(half(&board, PAGES + 0x1000), 1);
    assert_eq!(word(&board, PAGES + 0x1010), 2001);
    let line = |board: &Session| {
        let pending = &board.interrupt_state().unwrap()[0].pending;
        pending[(IRQ / 32) as usize] & 1 << (IRQ % 32) != 0
    };
    assert!(line(&board), "level line pending");

    board.write(RESULT + 12, &1u32.to_le_bytes()).unwrap();
    board.run_board(4000, 100).unwrap();
    assert!(board.state(false).failure.is_none());
    assert_eq!(word(&board, RESULT + 8), 0, "acknowledged");
    assert!(!line(&board), "level line lowered");
}
