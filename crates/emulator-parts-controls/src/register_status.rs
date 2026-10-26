//! A bounded register-byte endpoint with independently declared status replies.
//!
//! Configurations are opaque bytes. Initializer examples do not establish
//! writable-bit masks or grant new status effects. Only an explicit completed
//! register/data command changes the external status frame.
use fujimod_emulator_devices::{Unsupported, WordMap, record::Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub register: u8,
    pub data: u8,
    pub frame: [u8; 2],
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub address: u8,
    pub writable_registers: Vec<u8>,
    pub status: [u8; 2],
    pub transitions: Vec<Transition>,
}
pub struct Wiring;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterStatus {
    pub address: u8,
    writable: BTreeSet<u8>,
    pub registers: WordMap,
    pub frame: Vec<u8>,
    transitions: Vec<Transition>,
}
impl RegisterStatus {
    pub fn new(options: Options, _: Wiring) -> std::result::Result<Self, String> {
        let writable: BTreeSet<_> = options.writable_registers.iter().copied().collect();
        let mut commands = BTreeSet::new();
        if options.address > 0x7f
            || writable.is_empty()
            || writable.len() != options.writable_registers.len()
            || options
                .transitions
                .iter()
                .any(|r| !writable.contains(&r.register) || !commands.insert((r.register, r.data)))
        {
            return Err("invalid register-byte/status endpoint inputs".into());
        }
        Ok(Self {
            address: options.address,
            writable,
            registers: options
                .writable_registers
                .into_iter()
                .map(|r| (r.into(), 0))
                .collect(),
            frame: options.status.to_vec(),
            transitions: options.transitions,
        })
    }
    /// Preflight one transmitted byte without changing the endpoint.
    pub fn validate_byte(&self, payload: &[u8], byte: u8) -> Result<()> {
        match payload {
            [] if self.writable.contains(&byte) => Ok(()),
            [register] if self.writable.contains(register) => Ok(()),
            _ => Err(Unsupported(
                "Register/status endpoint requires one declared register and one data byte".into(),
            )),
        }
    }
    /// Called once after the controller completes STOP. Intermediate shifts
    /// and host observation cannot program a register or update status.
    pub fn complete_write(&mut self, payload: &[u8]) -> Result<Option<[u8; 2]>> {
        let [register, data] = payload else {
            return Err(Unsupported(
                "Register/status write requires exactly two bytes".into(),
            ));
        };
        self.validate_byte(&[], *register)?;
        self.validate_byte(&[*register], *data)?;
        self.registers.insert((*register).into(), (*data).into());
        let reply = self
            .transitions
            .iter()
            .find(|r| r.register == *register && r.data == *data)
            .map(|r| r.frame);
        if let Some(frame) = reply {
            self.frame = frame.to_vec();
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> RegisterStatus {
        RegisterStatus::new(
            Options {
                address: 0x44,
                writable_registers: vec![1, 2, 3, 4, 6],
                status: [0, 0],
                transitions: vec![
                    Transition {
                        register: 2,
                        data: 0x20,
                        frame: [0, 1],
                    },
                    Transition {
                        register: 2,
                        data: 0x40,
                        frame: [0, 0],
                    },
                ],
            },
            Wiring,
        )
        .unwrap()
    }
    #[test]
    fn stock_cfgdata_bytes_are_retained_without_inventing_status_effects() {
        // The model retains each supplied configuration byte without adding
        // status effects.
        let mut part = endpoint();
        for value in [0x29, 0x4d, 0x4a, 0xff] {
            part.validate_byte(&[], 2).unwrap();
            part.validate_byte(&[2], value).unwrap();
            assert_eq!(part.registers.get(2), Some(0));
            assert_eq!(part.frame, [0, 0]);
        }
        assert_eq!(part.complete_write(&[2, 0x4d]).unwrap(), None);
        assert_eq!(part.registers.get(2), Some(0x4d));
        assert_eq!(part.complete_write(&[2, 0x20]).unwrap(), Some([0, 1]));
        assert_eq!(part.complete_write(&[2, 0x4a]).unwrap(), None);
        assert_eq!(part.frame, [0, 1]); // 4A is not an alias for command40.
        assert_eq!(part.complete_write(&[2, 0x40]).unwrap(), Some([0, 0]));
    }
    #[test]
    fn unknown_registers_excess_bytes_and_ambiguous_inputs_refuse_without_effects() {
        let mut part = endpoint();
        for payload in [vec![], vec![2], vec![0, 1], vec![2, 1, 2]] {
            assert!(part.complete_write(&payload).is_err());
        }
        assert_eq!(part.registers.get(2), Some(0));
        assert_eq!(part.frame, [0, 0]);
        assert!(part.validate_byte(&[2, 1], 2).is_err());
        for (address, registers, transitions) in [
            (0x80, vec![2], vec![]),
            (0x44, vec![], vec![]),
            (0x44, vec![2, 2], vec![]),
            (
                0x44,
                vec![2],
                vec![Transition {
                    register: 1,
                    data: 0,
                    frame: [0, 0],
                }],
            ),
            (
                0x44,
                vec![2],
                vec![
                    Transition {
                        register: 2,
                        data: 0,
                        frame: [0, 0],
                    },
                    Transition {
                        register: 2,
                        data: 0,
                        frame: [0, 1],
                    },
                ],
            ),
        ] {
            assert!(
                RegisterStatus::new(
                    Options {
                        address,
                        writable_registers: registers,
                        status: [0, 0],
                        transitions
                    },
                    Wiring
                )
                .is_err()
            );
        }
    }
}
