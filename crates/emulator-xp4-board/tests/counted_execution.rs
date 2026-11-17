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

fn session(counted: bool) -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 65536, "cores": 2, "discovery": false, "cbar": CBAR,
        "labels": [BUMP],
        "initial_registers": [[["Pc", CORE0]], [["Pc", CORE1]]]
    }))
    .unwrap();
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
        let session = Session::new(definition, None, vec![]).unwrap();
        unsafe { std::env::remove_var("FUJIMOD_XP4_COUNTED") };
        session
    };
    assert_eq!(session.counted_execution(), counted);
    for (address, words) in PROGRAM {
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
