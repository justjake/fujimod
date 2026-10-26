//! Python values for explicit discovery observations.
use pyo3::prelude::*;

#[pyclass(get_all, frozen, module = "fujimod._native.emulator")]
#[derive(Clone, Debug)]
pub struct Intervention {
    pub kind: String,
    pub cpu: usize,
    pub address: u64,
    pub pc: u32,
    pub size: u32,
    pub before: u64,
    pub value: u64,
    pub instructions: u64,
    pub outcome: String,
    pub evidence: String,
}

#[pyclass(get_all, frozen, module = "fujimod._native.emulator")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontierEntry {
    pub first: u64,
    pub count: u64,
    pub cpu: usize,
    pub address: u64,
    pub pc: u32,
    pub size: u32,
    pub write: bool,
    pub value: u64,
    pub kind: String,
}

impl From<fujimod_emulator::board::discovery::Intervention> for Intervention {
    fn from(value: fujimod_emulator::board::discovery::Intervention) -> Self {
        let fujimod_emulator::board::discovery::Intervention {
            kind,
            cpu,
            address,
            pc,
            size,
            before,
            value,
            instructions,
            outcome,
            evidence,
        } = value;
        Self {
            kind,
            cpu,
            address,
            pc,
            size,
            before,
            value,
            instructions,
            outcome,
            evidence,
        }
    }
}

impl From<fujimod_emulator::board::discovery::FrontierEntry> for FrontierEntry {
    fn from(value: fujimod_emulator::board::discovery::FrontierEntry) -> Self {
        let fujimod_emulator::board::discovery::FrontierEntry {
            first,
            count,
            cpu,
            address,
            pc,
            size,
            write,
            value,
            kind,
        } = value;
        Self {
            first,
            count,
            cpu,
            address,
            pc,
            size,
            write,
            value,
            kind,
        }
    }
}
