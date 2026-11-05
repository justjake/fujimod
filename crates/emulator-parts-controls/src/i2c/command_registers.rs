//! Bounded command-byte register parts: configuration writes and external read frames.
//! Command masks, writable bits and read inputs are body data, never program state.
use fujimod_emulator_devices::{WordMap, record::Result};

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(fujimod_emulator_devices::Unsupported(message.into()))
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CommandRegisters {
    pub address: u8,
    pub prefix_mask: u8,
    pub prefix_value: u8,
    pub register_mask: u8,
    pub write_masks: WordMap,
    pub registers: WordMap,
    pub read_frames: Vec<(u8, Vec<u8>)>,
    pub commands: Vec<u8>,
    /// The register selected by the last accepted pointer write; a read
    /// transaction without its own pointer returns this register's frame.
    pub pointer: Option<u8>,
}

impl CommandRegisters {
    fn register(&self, prefix: u8) -> Result<u32> {
        if prefix & self.prefix_mask != self.prefix_value
            || prefix & !(self.prefix_mask | self.register_mask) != 0
        {
            return fail("Command-register target rejects prefix bits");
        }
        Ok((prefix & self.register_mask).into())
    }

    pub fn read(&self, payload: &[u8]) -> Result<Vec<u8>> {
        let [prefix] = payload else {
            return fail("Command-register read requires one prefix byte");
        };
        let register = self.register(*prefix)?;
        self.read_frames
            .iter()
            .find(|row| u32::from(row.0) == register)
            .map(|row| row.1.clone())
            .ok_or_else(|| {
                fujimod_emulator_devices::Unsupported("Command-register read has no declared input frame".into())
            })
    }

    /// Frame of the register selected by the last pointer write.
    pub fn read_pointer(&self) -> Result<Vec<u8>> {
        match self.pointer {
            Some(prefix) => self.read(&[prefix]),
            None => fail("Command-register read has no selected register"),
        }
    }

    pub fn validate_byte(&self, payload: &[u8], byte: u8) -> Result<()> {
        if payload.is_empty() {
            if self.commands.contains(&byte) {
                return Ok(());
            }
            let register = self.register(byte)?;
            if self.write_masks.get(register).is_some()
                || self
                    .read_frames
                    .iter()
                    .any(|row| u32::from(row.0) == register)
            {
                return Ok(());
            }
            return fail("Command-register prefix selects an undeclared register");
        }
        if self.commands.contains(&payload[0]) {
            return fail("Standalone command rejects trailing bytes");
        }
        let register = self.register(payload[0])? + payload.len() as u32 - 1;
        if register > u32::from(self.register_mask) {
            return fail("Command-register burst exceeds its register range");
        }
        let Some(mask) = self.write_masks.get(register) else {
            return fail(
                "Command-register write selects a read-only or undeclared register",
            );
        };
        if u32::from(byte) & !mask != 0 {
            return fail("Command-register write contains undeclared data bits");
        }
        Ok(())
    }

    pub fn validate_write(&self, payload: &[u8]) -> Result<()> {
        if payload.len() == 1 && self.commands.contains(&payload[0]) {
            return Ok(());
        }
        if let [prefix] = payload {
            // A register-pointer write: the next read transaction returns the
            // declared frame for this register.
            let register = self.register(*prefix)?;
            if self.write_masks.get(register).is_some()
                || self.read_frames.iter().any(|row| u32::from(row.0) == register)
            {
                return Ok(());
            }
            return fail("Command-register pointer selects an undeclared register");
        }
        if payload.len() < 2 {
            return fail("Command-register write requires data or a standalone command");
        }
        for (index, byte) in payload.iter().enumerate() {
            self.validate_byte(&payload[..index], *byte)?;
        }
        Ok(())
    }

    pub fn complete_write(&mut self, payload: &[u8]) -> Result<()> {
        self.validate_write(payload)?;
        if let [byte] = payload {
            if !self.commands.contains(byte) {
                self.pointer = Some(*byte);
            }
            return Ok(());
        }
        let first = self.register(payload[0])?;
        self.pointer = Some(payload[0]);
        for (index, byte) in payload[1..].iter().enumerate() {
            self.registers
                .insert(first + index as u32, u32::from(*byte));
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub address: u8,
    pub prefix_mask: u8,
    pub prefix_value: u8,
    pub register_mask: u8,
    pub write_masks: WordMap,
    pub read_frames: Vec<(u8, Vec<u8>)>,
    pub commands: Vec<u8>,
}
impl CommandRegisters {
    pub fn new(options: Options) -> std::result::Result<Self, String> {
        let Options {
            address,
            prefix_mask,
            prefix_value,
            register_mask,
            write_masks,
            read_frames,
            commands,
        } = options;
        if address > 0x7f
            || prefix_mask & register_mask != 0
            || prefix_value & !prefix_mask != 0
            || prefix_mask | register_mask != 0xff
            || u16::from(register_mask) & (u16::from(register_mask) + 1) != 0
            || write_masks.is_empty()
            || write_masks
                .items()
                .any(|(r, m)| r > u32::from(register_mask) || m > 255)
            || read_frames.iter().any(|(r, b)| {
                b.is_empty() || usize::from(*r) + b.len() > usize::from(register_mask) + 1
            })
            || read_frames
                .iter()
                .enumerate()
                .any(|(i, r)| read_frames[..i].iter().any(|p| p.0 == r.0))
            || commands
                .iter()
                .enumerate()
                .any(|(i, c)| commands[..i].contains(c) || *c & prefix_mask == prefix_value)
        {
            return Err("invalid command-register part contract".into());
        }
        let registers = write_masks.items().map(|(r, _)| (r, 0)).collect();
        Ok(Self {
            address,
            prefix_mask,
            prefix_value,
            register_mask,
            write_masks,
            registers,
            read_frames,
            commands,
            pointer: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part() -> CommandRegisters {
        CommandRegisters::new(Options {
            address: 0x30,
            prefix_mask: 0x80,
            prefix_value: 0,
            register_mask: 0x7f,
            write_masks: [(0x20, 0xff)].into_iter().collect(),
            read_frames: vec![(0x20, vec![0, 0, 0, 0x80])],
            commands: vec![],
        })
        .unwrap()
    }

    #[test]
    fn a_read_without_its_own_pointer_returns_the_selected_frame() {
        let mut part = part();
        assert!(part.read_pointer().is_err());
        part.complete_write(&[0x20]).unwrap();
        assert_eq!(part.read_pointer().unwrap(), vec![0, 0, 0, 0x80]);
    }

    #[test]
    fn a_declared_register_pointer_is_accepted_alone() {
        let part = part();
        assert!(part.validate_write(&[0x20]).is_ok());
        assert_eq!(part.read(&[0x20]).unwrap(), vec![0, 0, 0, 0x80]);
    }

    #[test]
    fn an_undeclared_register_pointer_is_refused() {
        assert!(part().validate_write(&[0x21]).is_err());
    }
}
