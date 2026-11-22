//! Counted execution (`FUJIMOD_XP4_COUNTED=1`) must keep the execution law of
//! the per-instruction hook: the same RAM, counters, trace, calls and
//! translation-maintenance deliveries, at every scheduler quantum.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};
use std::sync::Mutex;

const CBAR: u32 = 0x2000_0000;
const CORE0: u32 = 0x1000;
const CORE1: u32 = 0x2000;
const BUMP: u32 = 0x200;
const SHARED: usize = 0x3000;

// The environment is process-wide: sessions are opened one at a time.
static ENVIRONMENT: Mutex<()> = Mutex::new(());

/// Two cores. Core 0 enables the distributor, then 200 times: increments a
/// shared word with LDREX/STREX, calls the labeled `bump`, sends SGI 3 to
/// core 1 every eighth time and issues TLBIALLIS; then WFI. Core 1 enables
/// its CPU interface, unmasks IRQ and, 150 times, increments the shared word
/// and reads MPIDR; its IRQ handler acknowledges the SGI and counts it with
/// LDREX/STREX. Both end in WFI.
const PROGRAM: &[(u32, &[u32])] = &[
    (0x18, &[0xea00_0038]), // b irq
    (
        0x100,
        &[
            0xe302_8000, // irq: movw r8, #0x2000
            0xe342_8000, // movt r8, #0x2000 (GICC)
            0xe598_900c, // ldr r9, [r8, #0xc] (IAR)
            0xe588_9010, // str r9, [r8, #0x10] (EOIR)
            0xe303_a000, // movw r10, #0x3000
            0xe19a_bf9f, // 1: ldrex r11, [r10]
            0xe28b_b001, // add r11, r11, #1
            0xe18a_cf9b, // strex r12, r11, [r10]
            0xe35c_0000, // cmp r12, #0
            0x1aff_fffa, // bne 1b
            0xe25e_f004, // subs pc, lr, #4
        ],
    ),
    (
        BUMP,
        &[
            0xe080_0001, // bump: add r0, r0, r1
            0xe12f_ff1e, // bx lr
        ],
    ),
    (
        CORE0,
        &[
            0xe301_0000, // movw r0, #0x1000
            0xe342_0000, // movt r0, #0x2000 (GICD)
            0xe3a0_1001, // mov r1, #1
            0xe580_1000, // str r1, [r0] (CTLR)
            0xe303_2004, // movw r2, #0x3004
            0xe3a0_6000, // mov r6, #0
            0xe301_7f00, // movw r7, #0x1f00
            0xe342_7000, // movt r7, #0x2000 (SGIR)
            0xe300_5003, // movw r5, #3
            0xe340_5002, // movt r5, #2 (SGI 3 to core 1)
            0xe192_3f9f, // 2: ldrex r3, [r2]
            0xe283_3001, // add r3, r3, #1
            0xe182_4f93, // strex r4, r3, [r2]
            0xe354_0000, // cmp r4, #0
            0x1aff_fffa, // bne 2b
            0xe1a0_0006, // mov r0, r6
            0xe3a0_1003, // mov r1, #3
            0xebff_fc6d, // bl bump
            0xe582_0004, // str r0, [r2, #4]
            0xe316_0007, // tst r6, #7
            0x0587_5000, // streq r5, [r7]
            0xe3a0_0000, // mov r0, #0
            0xee08_0f13, // mcr p15, 0, r0, c8, c3, 0 (TLBIALLIS)
            0xe286_6001, // add r6, r6, #1
            0xe356_00c8, // cmp r6, #200
            0xbaff_ffef, // blt 2b
            0xe320_f003, // 3: wfi
            0xeaff_fffd, // b 3b
        ],
    ),
    (
        CORE1,
        &[
            0xe302_0000, // movw r0, #0x2000
            0xe342_0000, // movt r0, #0x2000 (GICC)
            0xe3a0_1001, // mov r1, #1
            0xe580_1000, // str r1, [r0] (CTLR)
            0xe3a0_10ff, // mov r1, #0xff
            0xe580_1004, // str r1, [r0, #4] (PMR)
            0xf108_0080, // cpsie i
            0xe303_2004, // movw r2, #0x3004
            0xe192_3f9f, // 4: ldrex r3, [r2]
            0xe283_3001, // add r3, r3, #1
            0xe182_4f93, // strex r4, r3, [r2]
            0xe354_0000, // cmp r4, #0
            0x1aff_fffa, // bne 4b
            0xee10_5fb0, // mrc p15, 0, r5, c0, c0, 5 (MPIDR)
            0xe592_600c, // ldr r6, [r2, #0xc]
            0xe286_6001, // add r6, r6, #1
            0xe582_600c, // str r6, [r2, #0xc]
            0xe356_0096, // cmp r6, #150
            0xbaff_fff4, // blt 4b
            0xe320_f003, // 5: wfi
            0xeaff_fffd, // b 5b
        ],
    ),
];

fn open(definition: Definition, nor: Option<Vec<u8>>, counted: bool) -> Session {
    let session = {
        let _environment = ENVIRONMENT.lock().unwrap();
        // SAFETY: sessions in this binary are opened under the lock above.
        unsafe {
            if counted {
                std::env::set_var("FUJIMOD_XP4_COUNTED", "1");
            } else {
                std::env::remove_var("FUJIMOD_XP4_COUNTED");
            }
        }
        let session = Session::new(definition, nor, vec![]).unwrap();
        unsafe { std::env::remove_var("FUJIMOD_XP4_COUNTED") };
        session
    };
    assert_eq!(session.counted_execution(), counted);
    session
}

fn session(counted: bool) -> Session {
    session_with(counted, PROGRAM)
}

fn session_with(counted: bool, program: &[(u32, &[u32])]) -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 65536, "cores": 2, "discovery": false, "cbar": CBAR,
        "labels": [BUMP],
        "initial_registers": [[["Pc", CORE0]], [["Pc", CORE1]]]
    }))
    .unwrap();
    let session = open(definition, None, counted);
    for (address, words) in program {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        session.write(*address as usize, &bytes).unwrap();
    }
    session
}

/// Everything a run leaves: the full state report, the maintenance record
/// and the whole RAM.
fn outcome(session: &Session) -> (Value, String, Vec<u8>) {
    let mut ram = vec![0; session.ram_bytes()];
    session.read(0, &mut ram).unwrap();
    (
        serde_json::to_value(session.state(true)).unwrap(),
        anonymous(&format!("{:?}", session.translation_maintenance_state())),
        ram,
    )
}

/// The maintenance record without processor identities and owner tokens,
/// which are unique per opened engine.
fn anonymous(record: &str) -> String {
    let mut out = String::with_capacity(record.len());
    let mut rest = record;
    while let Some(at) = ["Identity(", "owner: "]
        .iter()
        .filter_map(|key| rest.find(key).map(|i| i + key.len()))
        .min()
    {
        out.push_str(&rest[..at]);
        rest = rest[at..].trim_start_matches(|c: char| c.is_ascii_digit());
        out.push('_');
    }
    out + rest
}

#[test]
fn counted_runs_match_the_per_instruction_hook_at_every_quantum() {
    for quantum in [1, 7, 64, 1000, 20_000] {
        let mut outcomes = Vec::new();
        for counted in [false, true] {
            let session = session(counted);
            // Several board calls, so slices also end at call boundaries.
            for _ in 0..4 {
                session.run_board(2500, quantum).unwrap();
            }
            let state = session.state(false);
            assert!(
                state.failure.is_none(),
                "quantum {quantum}, counted {counted}: {:?}",
                state.failure
            );
            outcomes.push(outcome(&session));
        }
        let (exact, counted) = (&outcomes[0], &outcomes[1]);
        // The program ran: shared increments, IRQs and labeled calls.
        let word = |at: usize| u32::from_le_bytes(exact.2[at..at + 4].try_into().unwrap());
        assert_eq!(word(SHARED + 4), 200 + 150, "quantum {quantum}");
        assert!(word(SHARED) > 0, "quantum {quantum}: core 1 took no IRQ");
        assert_eq!(exact.0["history"]["calls"].as_array().unwrap().len(), 200);
        assert_eq!(exact.0, counted.0, "state at quantum {quantum}");
        assert_eq!(
            exact.1, counted.1,
            "translation maintenance at quantum {quantum}"
        );
        assert!(exact.2 == counted.2, "RAM at quantum {quantum}");
    }
}

#[test]
fn counted_sessions_refuse_observers_that_need_every_instruction() {
    let session = session(true);
    assert!(
        session
            .memory_watch_begin(vec![(0, 0x1000)], 16, false)
            .is_err()
    );
    assert!(
        session
            .instruction_points(0, vec![u64::from(CORE0)])
            .is_err()
    );
    assert!(session.run_to(0, 16, u64::from(CORE0) + 8).is_err());
    assert!(session.instruction_points(0, vec![]).is_ok());
}

/// One core that calls code in the serial-flash aperture, executed in place,
/// then changes what the aperture reads (a controller read state, or a sector
/// erase that completes on a device deadline) and calls the same page again.
/// The scenarios of `nor_execute_in_place.rs`.
mod execute_in_place {
    use super::*;

    const CONTROLLER: u32 = 0xfffd_8000;
    const PAD: u32 = 0xfff4_108c;
    const FLASH_CODE: u32 = 0xf000_1100;
    const MARKERS: usize = 0x8000;
    const CONFIGURE: [u32; 5] = [
        0xe581_4000, // str r4, [r1]          control
        0xe582_5000, // str r5, [r2]          pad function
        0xe581_6004, // str r6, [r1, #4]      clock
        0xe581_7014, // str r7, [r1, #0x14]   mode
        0xe581_8010, // str r8, [r1, #0x10]   sample point
    ];
    const CALL: u32 = 0xe12f_ff33; // blx r3: into the aperture
    const WFI: u32 = 0xe320_f003;

    fn definition() -> Definition {
        let registers = [
            (1, CONTROLLER),
            (2, PAD),
            (3, FLASH_CODE),
            (4, 0x80041),
            (5, 0xffff_5545),
            (6, 0x13),
            (7, 0x10_1003),
            (8, 9),
            (9, 5),
            (11, MARKERS as u32),
            (15, 0),
        ];
        serde_json::from_value(json!({
            "ram_size": 65536, "cores": 1, "discovery": false, "cbar": 0xffff_0000u32,
            "storage_jedec": 0x20_2018, "storage_dma_routes": (32..48).collect::<Vec<_>>(),
            "storage_geometry": [4096, 256, 10, 2], "storage_irq": 48,
            "gpio": vec![0u16; 16],
            "storage_mapped_read": {
                "states": [{"control": [0x78_0071, 0x80041], "clock": 0x13, "mode": 0x10_1003,
                    "samples": [9, 11], "pad": true}],
                "miss_byte": 0xa5, "pad": [PAD, 0xffff, 0x5545]},
            "storage_write_protect": false,
            "initial_registers": [registers.iter()
                .map(|&(r, v)| json!([{"Arm32": {"R": r}}, v])).collect::<Vec<_>>()]
        }))
        .unwrap()
    }

    /// The array: `bx lr` after setting both markers, at the aperture's code page.
    fn nor() -> Vec<u8> {
        let mut nor = vec![0xff; 8192];
        let code = [0xe3a0_0055u32, 0xe58b_0000, 0xe58b_0004, 0xe12f_ff1e];
        for (i, word) in code.iter().enumerate() {
            nor[0x1100 + 4 * i..0x1100 + 4 * i + 4].copy_from_slice(&word.to_le_bytes());
        }
        nor
    }

    /// `ldr r0, [pc, #offset]` at word `at`, reading the literal word at `literal`.
    fn ldr_r0(at: usize, literal: usize) -> u32 {
        0xe59f_0000 | ((literal - at) as u32 * 4 - 8)
    }

    fn controller_change() -> Vec<u32> {
        let mut program = CONFIGURE.to_vec();
        program.extend([
            CALL,
            0xe581_9010, // str r9, [r1, #0x10]  a sample point outside the window
            0xe3a0_0000, // mov r0, #0
            0xe58b_0000, // str r0, [r11]        clear the first marker
            CALL,        // the page reads again, now as the miss pattern
            WFI,
        ]);
        program
    }

    fn erase() -> Vec<u32> {
        let wait = [0xe3a0_20c8u32, 0xe282_2b01, 0xe252_2001, 0x1aff_fffd];
        let literals = 100;
        let mut program = CONFIGURE.to_vec();
        program.push(CALL);
        program.push(0xe58b_0008); // str r0, [r11, #8]
        program.push(ldr_r0(program.len(), literals)); // write enable
        program.push(0xe581_0090); // str r0, [r1, #0x90]
        program.extend(wait);
        program.push(ldr_r0(program.len(), literals + 1)); // erase address
        program.push(0xe581_0094); // str r0, [r1, #0x94]
        program.push(ldr_r0(program.len(), literals + 2)); // four-kib erase
        program.push(0xe581_0090); // str r0, [r1, #0x90]
        program.extend(wait);
        program.extend([0xe3a0_0000, 0xe58b_0000, 0xe58b_0004, CALL, WFI]);
        program.resize(literals, WFI);
        program.extend([0x0600_0001, 0xf000_1000, 0x210b_0001]);
        program
    }

    /// Runs core 0 in calls of `chunk` instructions until one yields (enters
    /// fewer), as the single call of `nor_execute_in_place.rs` ends there.
    /// A chunk of one could not tell that yield from a full call.
    fn run(program: &[u32], counted: bool, chunk: usize) -> (Value, String, Vec<u8>) {
        let session = open(definition(), Some(nor()), counted);
        let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
        session.write(0, &bytes).unwrap();
        let entered = |s: &Session| {
            serde_json::to_value(s.state(false)).unwrap()["instructions"]
                .as_u64()
                .unwrap()
        };
        let mut total = 0;
        while total < 200_000 {
            let before = entered(&session);
            let result = session.run(0, chunk);
            let now = entered(&session);
            total = now;
            if result.is_err() || now - before < chunk as u64 {
                break;
            }
        }
        outcome(&session)
    }

    #[test]
    fn counted_runs_drop_execute_in_place_copies_where_the_hook_does() {
        // Markers [first, second, proof of the first call]. After the erase
        // the second call fetches 0xffffffff and fails as invalid: both
        // modes must fail there alike.
        for (name, program, markers) in [
            ("controller read state", controller_change(), [0, 0x55, 0]),
            ("completed erase", erase(), [0, 0, 0x55]),
        ] {
            for chunk in [7, 64, 1000, 200_000] {
                let exact = run(&program, false, chunk);
                let counted = run(&program, true, chunk);
                let word = |at: usize| u32::from_le_bytes(exact.2[at..at + 4].try_into().unwrap());
                assert_eq!(
                    [word(MARKERS), word(MARKERS + 4), word(MARKERS + 8)],
                    markers,
                    "{name}, chunk {chunk}"
                );
                assert_eq!(exact.0, counted.0, "{name}: state, chunk {chunk}");
                assert_eq!(exact.1, counted.1, "{name}: maintenance, chunk {chunk}");
                assert!(exact.2 == counted.2, "{name}: RAM, chunk {chunk}");
            }
        }
    }
}

/// Two cores contend for one word: core 0 increments it with LDREX/STREX and
/// counts its failed STREXes; core 1 increments its second byte with
/// LDREXB/STREXB (a different reservation granule that overlaps core 0's), counts
/// its failures, and between its attempts stores to a byte of the word (which
/// clears core 0's reservation) and to a word beside it (which does not).
const CONTENTION: &[(u32, &[u32])] = &[
    (
        CORE0,
        &[
            0xe303_2000, // movw r2, #0x3000
            0xe303_b010, // movw r11, #0x3010
            0xe3a0_6000, // mov r6, #0
            0xe3a0_8000, // mov r8, #0
            0xe192_3f9f, // 1: ldrex r3, [r2]
            0xe1a0_0000, // nop
            0xe1a0_0000, // nop
            0xe283_3001, // add r3, r3, #1
            0xe182_4f93, // strex r4, r3, [r2]
            0xe354_0000, // cmp r4, #0
            0x1288_8001, // addne r8, r8, #1
            0x1aff_fff7, // bne 1b
            0xe286_6001, // add r6, r6, #1
            0xe356_0f4b, // cmp r6, #300
            0xbaff_fff4, // blt 1b
            0xe58b_8000, // str r8, [r11]
            0xe58b_6004, // str r6, [r11, #4]
            0xe320_f003, // 2: wfi
            0xeaff_fffd, // b 2b
        ],
    ),
    (
        CORE1,
        &[
            0xe303_2000, // movw r2, #0x3000
            0xe303_a001, // movw r10, #0x3001
            0xe303_b020, // movw r11, #0x3020
            0xe3a0_9000 | 0x5a, // mov r9, #0x5a
            0xe3a0_6000, // mov r6, #0
            0xe3a0_8000, // mov r8, #0
            0xe1da_3f9f, // 1: ldrexb r3, [r10]
            0xe1a0_0000, // nop
            0xe283_3001, // add r3, r3, #1
            0xe1ca_4f93, // strexb r4, r3, [r10]
            0xe354_0000, // cmp r4, #0
            0x1288_8001, // addne r8, r8, #1
            0x1aff_fff8, // bne 1b
            0xe5c2_9002, // strb r9, [r2, #2]
            0xe582_9008, // str r9, [r2, #8]
            0xe286_6001, // add r6, r6, #1
            0xe356_0f4b, // cmp r6, #300
            0xbaff_fff3, // blt 1b
            0xe58b_8000, // str r8, [r11]
            0xe58b_6004, // str r6, [r11, #4]
            0xe320_f003, // 2: wfi
            0xeaff_fffd, // b 2b
        ],
    ),
];

#[test]
fn contended_exclusives_match_the_per_instruction_hook_at_every_quantum() {
    for quantum in [1, 2, 3, 5, 7, 64, 1000] {
        let mut outcomes = Vec::new();
        for counted in [false, true] {
            let session = session_with(counted, CONTENTION);
            for _ in 0..8 {
                session.run_board(2500, quantum).unwrap();
            }
            let state = session.state(false);
            assert!(state.failure.is_none(), "quantum {quantum}, counted {counted}: {:?}", state.failure);
            outcomes.push(outcome(&session));
        }
        let (exact, counted) = (&outcomes[0], &outcomes[1]);
        let word = |at: usize| u32::from_le_bytes(exact.2[at..at + 4].try_into().unwrap());
        assert_eq!(word(0x3014), 300, "quantum {quantum}: core 0 did not finish");
        assert_eq!(word(0x3024), 300, "quantum {quantum}: core 1 did not finish");
        if quantum <= 7 {
            assert!(
                word(0x3010) + word(0x3020) > 0,
                "quantum {quantum}: no STREX failed"
            );
        }
        assert_eq!(exact.0, counted.0, "state at quantum {quantum}");
        assert!(exact.2 == counted.2, "RAM at quantum {quantum}");
    }
}
