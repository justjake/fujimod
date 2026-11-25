//! XP4 device tests on the shared board: a session built from a bare
//! definition, with the physical-memory and register access the tests use.
#![allow(dead_code)]
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator_cpu::Register;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp4_board::{Definition, shared};
use serde_json::Value;

pub struct Board(pub Session);

/// The session a definition JSON builds, with no storage or SD card media.
pub fn board(value: Value) -> Result<Board, String> {
    let definition: Definition = serde_json::from_value(value).map_err(|e| e.to_string())?;
    shared::from_definition(definition, None, Vec::new(), Vec::new(), RunEngine::Unicorn).map(Board)
}

impl Board {
    pub fn write(&self, address: usize, bytes: &[u8]) -> Result<(), String> {
        self.0.board.write_physical(address as u64, bytes)
    }
    pub fn read(&self, address: usize, bytes: &mut [u8]) -> Result<(), String> {
        self.0.board.read_physical(address as u64, bytes)
    }
    pub fn word(&self, address: usize) -> u32 {
        let mut bytes = [0; 4];
        self.read(address, &mut bytes).unwrap();
        u32::from_le_bytes(bytes)
    }
    pub fn write_register(&self, core: usize, register: Register, value: u64) -> Result<(), String> {
        self.0.board.write_register(core, register, value)
    }
    pub fn read_register(&self, core: usize, register: Register) -> Result<u64, String> {
        self.0.board.read_register(core, register)
    }
    /// Run at most `count` instructions in slices of `quantum`.
    pub fn run(&mut self, count: u64, quantum: usize) -> Result<(), String> {
        self.0.board.run_for_instructions(count, quantum)
    }
    /// Saved states of every device of a kind, in registration order.
    pub fn states(&self, kind: &str) -> Vec<Value> {
        match self.0.board.device_states(kind).unwrap() {
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
        self.0.board.gic_state().unwrap()["pending"][0][line as usize] == true
    }
}
