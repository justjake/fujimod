//! XP4 device tests on the shared board: a session built from a bare
//! definition, with the physical-memory, register and device-state access
//! the tests use.
#![allow(dead_code)]
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Register;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp4_board::{Definition, shared};
use serde_json::Value;
use std::{cell::RefCell, collections::BTreeMap};

pub struct Board(pub RefCell<Session>);

/// The session a definition builds, with no storage or SD card media.
pub fn open(definition: Definition) -> Result<Board, String> {
    open_with(definition, None, Vec::new(), Vec::new())
}

/// The session a definition builds with its storage image and SD card inputs.
pub fn open_with(
    definition: Definition,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
) -> Result<Board, String> {
    shared::from_definition(definition, storage_image, card_files, card_media, RunEngine::Unicorn)
        .map(|session| Board(RefCell::new(session)))
}

/// The session a definition JSON builds.
pub fn board(value: Value) -> Result<Board, String> {
    open(serde_json::from_value(value).map_err(|e| e.to_string())?)
}

/// GIC distributor pending words, one per 32 lines.
pub struct Interrupts {
    pub pending: Vec<u32>,
}

impl Board {
    pub fn write(&self, address: usize, bytes: &[u8]) -> Result<(), String> {
        self.0.borrow().board.write_physical(address as u64, bytes)
    }
    pub fn read(&self, address: usize, bytes: &mut [u8]) -> Result<(), String> {
        self.0.borrow().board.read_physical(address as u64, bytes)
    }
    pub fn word(&self, address: usize) -> u32 {
        let mut bytes = [0; 4];
        self.read(address, &mut bytes).unwrap();
        u32::from_le_bytes(bytes)
    }
    pub fn write_register(&self, core: usize, register: Register, value: u64) -> Result<(), String> {
        self.0.borrow().board.write_register(core, register, value)
    }
    pub fn read_register(&self, core: usize, register: Register) -> Result<u64, String> {
        self.0.borrow().board.read_register(core, register)
    }
    /// Run at most `count` instructions. The shared board fixes one
    /// processor quantum per run; these short programs use one instruction.
    pub fn run_board(&self, count: u64, _quantum: usize) -> Result<(), String> {
        self.0.borrow_mut().board.run_for_instructions(count, 1)
    }
    /// Run at most `budget` instructions.
    pub fn run(&self, _core: usize, budget: usize) -> Result<(), String> {
        self.run_board(budget as u64, 1)
    }
    /// Instructions core 0 has retired.
    pub fn instructions(&self) -> u64 {
        self.0.borrow().board.instruction_counts()[0]
    }
    /// The board's retained execution failure.
    pub fn failure(&self) -> Option<String> {
        self.0.borrow().board.failure().unwrap()
    }
    /// A copy of the medium in the `host`th SD card slot.
    pub fn card_medium(&self, host: usize) -> Result<fujimod_emulator_devices::parts::sd_card::Card, String> {
        shared::card_medium(&self.0.borrow(), host)
    }
    /// Completed NOR contents.
    pub fn nor_medium(&self) -> Result<Vec<u8>, String> {
        shared::nor_medium(&self.0.borrow())
    }
    /// Every device's saved state, keyed by its node name (`kind`, or
    /// `kind#n` when the board has several of a kind).
    pub fn device_state_json(&self) -> BTreeMap<String, Value> {
        let names: Vec<String> = self.0.borrow().nodes.keys().cloned().collect();
        names
            .into_iter()
            .filter_map(|name| {
                let (kind, n) = name
                    .split_once('#')
                    .map_or((name.as_str(), 0), |(kind, n)| (kind, n.parse().unwrap()));
                self.states(kind).get(n).cloned().map(|state| (name.clone(), state))
            })
            .collect()
    }
    /// Saved states of every device of a kind, in registration order.
    pub fn states(&self, kind: &str) -> Vec<Value> {
        match self.0.borrow().board.device_states(kind).unwrap() {
            Value::Array(states) => states,
            other => panic!("{kind}: {other}"),
        }
    }
    /// The saved state of the one device of a kind.
    pub fn state(&self, kind: &str) -> Value {
        let mut states = self.states(kind);
        assert_eq!(states.len(), 1, "{kind}");
        states.remove(0)
    }
    /// Whether the GIC holds shared interrupt `line` pending.
    pub fn pending(&self, line: u32) -> bool {
        self.0.borrow().board.gic_state().unwrap()["pending"][0][line as usize] == true
    }
    /// The GIC's pending shared lines as distributor words.
    pub fn interrupt_state(&self) -> Result<Vec<Interrupts>, String> {
        let gic = self.0.borrow().board.gic_state()?;
        let lines = gic["pending"][0].as_array().ok_or("GIC state has no pending lines")?;
        let mut pending = vec![0u32; lines.len().div_ceil(32)];
        for (line, value) in lines.iter().enumerate() {
            if value == true {
                pending[line / 32] |= 1 << (line % 32);
            }
        }
        Ok(vec![Interrupts { pending }])
    }
}
