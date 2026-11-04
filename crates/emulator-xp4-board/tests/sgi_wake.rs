use fujimod_emulator_cpu::arm;
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::json;

const CBAR: u32 = 0x2000_0000;
const RECEIVER: usize = 0x1000;
const SENDER: usize = 0x2000;
const RESULT: usize = 0x4000;

fn program(session: &Session, address: usize, words: &[u32]) {
    let bytes: Vec<_> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(address, &bytes).unwrap();
}

#[test]
fn masked_wfi_wakes_only_when_the_target_cpu_interface_admits_the_sgi() {
    for (cpu_control, priority_mask, wakes) in [(1, 255, true), (0, 255, false), (1, 0, false)] {
        let definition: Definition = serde_json::from_value(json!({
            "ram_size":65536, "cores":2, "discovery":false, "cbar":CBAR,
            "initial_registers":[[["Pc",SENDER]], [["Pc",RECEIVER]]]
        })).unwrap();
        let session = Session::new(definition, None, vec![]).unwrap();
        // The receiver enables the distributor, configures its banked CPU
        // interface and PMR, masks IRQ exceptions, then executes WFI.
        program(&session, RECEIVER, &[
            0xe3a0_4001, 0xe580_4000, // MOV r4,#1; STR r4,[r0].
            0xe3a0_4000 | cpu_control, 0xe581_4000,
            0xe3a0_4000 | priority_mask, 0xe582_4000,
            0xf10c_0080, // CPSID i.
            0xe320_f003, // WFI.
            0xe3a0_402a, 0xe583_4000, // Publish 42 after wake.
            0xeaff_fffe, // B .
        ]);
        for (register, value) in [(0, CBAR + 0x1000), (1, CBAR + 0x2000),
            (2, CBAR + 0x2004), (3, RESULT as u32)] {
            session.write_register(1, arm::CORE[register], value.into()).unwrap();
        }
        // SGIR target-list-filter=1 sends SGI 3 to all other cores.
        program(&session, SENDER, &[0xe580_1000, 0xe320_f003]);
        session.write_register(0, arm::CORE[0], (CBAR + 0x1f00).into()).unwrap();
        session.write_register(0, arm::CORE[1], 0x0100_0003).unwrap();
        session.run(1, 16).unwrap();
        assert!(session.state(false).waiting[1]);
        let resume_pc = (RECEIVER + 8 * 4) as u64;
        assert_eq!(session.read_register(1, arm::PC).unwrap(), resume_pc);
        assert_ne!(session.read_register(1, arm::CPSR).unwrap() & 0x80, 0);

        session.run(0, 8).unwrap();
        assert_eq!(session.state(false).waiting[1], !wakes,
            "CPU control={cpu_control}, PMR={priority_mask}");
        session.run_board(2, 2).unwrap();
        let mut result = [0; 4];
        session.read(RESULT, &mut result).unwrap();
        assert_eq!(u32::from_le_bytes(result), if wakes {42} else {0});
        assert_eq!(session.read_register(1, arm::PC).unwrap(),
            resume_pc + if wakes {8} else {0});
        assert_ne!(session.read_register(1, arm::CPSR).unwrap() & 0x80, 0);
        assert_eq!(session.state(false).irq_entries, [0, 0]);
        assert!(session.state(false).failure.is_none());
    }
}
