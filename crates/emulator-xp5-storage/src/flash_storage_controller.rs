//! Serial NOR flash-storage PIO/status and aperture mode.
use crate::flash_storage::Shared;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    active: bool,
    complete: bool,
    cursor: u32,
    tx: Vec<u8>,
    #[serde(default)]
    rx_fifo: std::collections::VecDeque<u8>,
    configuration_complete: bool,
}
/// One installed serial NOR part: its identity and the opcodes, address bytes
/// and dummy cycles the controller is allowed to issue to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// JEDEC 0x20bb20, a quad-I/O NOR part. Its aperture read setup is not
    /// yet checked by this model.
    Quad20bb20,
    /// JEDEC 0x2c5b1b, Micron MT35XU01GBBA: 128 MiB octal NOR, addressed with
    /// four address bytes.
    MicronMt35xu01g,
}
/// One read opcode served through the memory-mapped aperture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApertureRead {
    pub opcode: u8,
    pub address_bytes: u8,
    pub dummy_cycles: u8,
}
impl Part {
    pub fn from_jedec(jedec: u32) -> Result<Self, String> {
        match jedec {
            0x20bb20 => Ok(Self::Quad20bb20),
            0x2c5b1b => Ok(Self::MicronMt35xu01g),
            other => Err(format!("unsupported serial NOR part {other:#08x}")),
        }
    }
    pub fn jedec(self) -> u32 {
        match self {
            Self::Quad20bb20 => 0x20bb20,
            Self::MicronMt35xu01g => 0x2c5b1b,
        }
    }
    /// Commands the PIO path issues: status/identity, reads, programs and erases.
    fn pio(self, cmd: u8) -> Option<Command> {
        use Command::*;
        match (self, cmd) {
            (_, 0x06) => Some(WriteEnable),
            (_, 0x04) => Some(WriteDisable),
            (_, 0x05) => Some(Status),
            (_, 0x9f) => Some(Identity),
            (Self::Quad20bb20, 0x03 | 0x13 | 0x0b | 0x0c | 0xeb | 0xec) => Some(Read),
            (Self::Quad20bb20, 0x02 | 0x12 | 0x32 | 0x34) => Some(Program),
            (Self::Quad20bb20, 0x20 | 0x21) => Some(Erase(0x1000)),
            (Self::Quad20bb20, 0xd8 | 0xdc) => Some(Erase(0x10000)),
            // MT35XU: 4-byte read, octal-output and octal-I/O 4-byte fast reads,
            // 4-byte page program, 4 KiB and 128 KiB 4-byte erases.
            (Self::MicronMt35xu01g, 0x13 | 0xcc | 0xfd) => Some(Read),
            (Self::MicronMt35xu01g, 0x12) => Some(Program),
            (Self::MicronMt35xu01g, 0x21) => Some(Erase(0x1000)),
            (Self::MicronMt35xu01g, 0xdc) => Some(Erase(0x20000)),
            _ => None,
        }
    }
    /// Aperture read setups the part answers, or None while unchecked.
    pub fn aperture_reads(self) -> Option<&'static [ApertureRead]> {
        match self {
            Self::Quad20bb20 => None,
            // Octal-I/O and octal-output 4-byte fast reads with 20 dummy cycles.
            Self::MicronMt35xu01g => Some(&[
                ApertureRead { opcode: 0xfd, address_bytes: 4, dummy_cycles: 20 },
                ApertureRead { opcode: 0xcc, address_bytes: 4, dummy_cycles: 20 },
            ]),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    WriteEnable,
    WriteDisable,
    Status,
    Identity,
    Read,
    Program,
    Erase(usize),
}
pub struct Controller {
    state: State,
    flash_storage: Shared,
    part: Part,
}
impl Controller {
    pub fn new(flash_storage: Shared, part: Part) -> Self {
        let mut regs = vec![0; 64];
        regs[0x10 / 4] = 0x10001; // Explicit inherited enabled/idle controller assumption.
        Self {
            state: State {
                regs,
                active: false,
                complete: false,
                cursor: 0,
                tx: Vec::new(),
                rx_fifo: Default::default(),
                configuration_complete: false,
            },
            flash_storage,
            part,
        }
    }
    /// The aperture's configured read opcode and dummy cycles must be a read
    /// the installed part answers.
    fn check_aperture_read(&self) -> Result<(), String> {
        let Some(reads) = self.part.aperture_reads() else {
            return Ok(());
        };
        let opcode = self.state.regs[0x44 / 4] as u8;
        let dummy = self.state.regs[0x74 / 4];
        if reads
            .iter()
            .any(|r| r.opcode == opcode && u32::from(r.dummy_cycles) == dummy)
        {
            Ok(())
        } else {
            Err(format!(
                "serial NOR part {:#08x} has no aperture read {opcode:#x} with {dummy} dummy cycles",
                self.part.jedec()
            ))
        }
    }
    fn known(o: u64) -> bool {
        matches!(
            o,
            0 | 4
                | 8
                | 0x10
                | 0x14
                | 0x18
                | 0x20
                | 0x24
                | 0x34
                | 0x40
                | 0x44
                | 0x48
                | 0x50
                | 0x74
                | 0x80
                | 0x94
                | 0x98
                | 0xa0
                | 0xa4
                | 0xc0
                | 0xc4
                | 0xc8
                | 0xd0
                | 0xe0
                | 0xf0
                | 0xf4
                | 0xf8
                | 0xfc
        )
    }
    fn cmd(&self) -> u8 {
        if self.state.regs[0] & 3 == 1 {
            self.state.regs[0x40 / 4] as u8
        } else {
            (self.state.regs[0x40 / 4] >> 8) as u8
        }
    }
    fn begin(&mut self) -> Result<(), String> {
        let cmd = self.cmd();
        self.state.cursor = 0;
        self.state.tx.clear();
        self.state.active = true;
        self.state.complete = false;
        let command = self.part.pio(cmd).ok_or_else(|| {
            format!(
                "unsupported serial-flash storage command {cmd:#x} for part {:#08x}",
                self.part.jedec()
            )
        })?;
        let jedec = self.part.jedec();
        let mut flash_storage = self.flash_storage.lock().unwrap();
        flash_storage.session.controller_complete = false;
        match command {
            Command::WriteEnable => {
                flash_storage.session.write_enable = true;
                self.state.complete = true;
            }
            Command::WriteDisable => {
                flash_storage.session.write_enable = false;
                self.state.complete = true;
            }
            Command::Status | Command::Identity | Command::Read => {
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = match command {
                    Command::Status => {
                        vec![(flash_storage.session.write_enable as u8) << 1; count].into()
                    }
                    Command::Identity => (0..count)
                        .map(|i| if i < 3 { (jedec >> (16 - i * 8)) as u8 } else { 0xff })
                        .collect(),
                    _ => flash_storage
                        .read_device(self.state.regs[0x50 / 4] as u64, count)?
                        .into(),
                };
                self.state.complete = true;
            }
            Command::Program => {}
            Command::Erase(size) => {
                flash_storage.erase(self.state.regs[0x50 / 4] as u64, size)?;
                self.state.complete = true;
            }
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        if self.part.pio(self.cmd()) == Some(Command::Program) && !self.state.tx.is_empty() {
            self.flash_storage
                .lock()
                .unwrap()
                .program(self.state.regs[0x50 / 4] as u64, &self.state.tx)?;
        }
        self.state.active = false;
        self.state.complete = true;
        Ok(())
    }
    fn rx(&mut self) -> Result<u8, String> {
        // FIFO contents survive a later command/strobe; register configuration
        // is not the identity of data already clocked into the receive FIFO.
        if let Some(b) = self.state.rx_fifo.pop_front() {
            self.state.cursor += 1;
            return Ok(b);
        }
        if !self.state.active {
            return Err("flash storage FIFO read outside transaction".into());
        }
        let mut s = self.flash_storage.lock().unwrap();
        let cmd = self.cmd();
        let b = match self.part.pio(cmd) {
            Some(Command::Status) => (s.session.write_enable as u8) << 1,
            Some(Command::Identity) => {
                if self.state.cursor < 3 {
                    (self.part.jedec() >> (16 - self.state.cursor * 8)) as u8
                } else {
                    0xff
                }
            }
            Some(Command::Read) => s.read_device(
                self.state.regs[0x50 / 4] as u64 + self.state.cursor as u64,
                1,
            )?[0],
            _ => return Err(format!("flash storage FIFO read for command {cmd:#x}")),
        };
        self.state.cursor += 1;
        self.state.complete = true;
        Ok(b)
    }
}
impl Device for Controller {
    fn pollable(&self, offset: u64, size: u8) -> bool {
        offset == 0xff90a0f0 && size == 4
    }
    fn name(&self) -> &'static str {
        "serial-flash storage"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return 0;
        }
        match o {
            0x80 => match self.rx() {
                Ok(b) => b as u64,
                Err(e) => {
                    cx.fail(e);
                    0
                }
            },
            0xe0 => {
                let mode = if self.state.regs[0xfc / 4] & 1 != 0 {
                    0x88
                } else if self.state.regs[0x10 / 4] & 1 == 0 {
                    0x18
                } else {
                    8
                };
                mode | if self.state.configuration_complete {
                    1 << 23
                } else {
                    0
                }
            }
            0xf0 => {
                let receive = !self.state.rx_fifo.is_empty()
                    || (self.state.active
                        && matches!(
                            self.part.pio(self.cmd()),
                            Some(Command::Status | Command::Identity | Command::Read)
                        ));
                let complete = self.state.complete
                    || self
                        .flash_storage
                        .lock()
                        .unwrap()
                        .session
                        .controller_complete;
                4 | if receive { 2 } else { 0 } | complete as u64
            }
            0xd0 => 0,
            _ => self.state.regs[o as usize / 4] as u64,
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        let result = match o {
            0x80 => {
                if !self.state.active || self.part.pio(self.cmd()) != Some(Command::Program) {
                    Err("unsupported flash storage FIFO write".into())
                } else {
                    self.state.tx.push(v as u8);
                    if self.state.tx.len() == (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1 {
                        self.stop()
                    } else {
                        Ok(())
                    }
                }
            }
            0xc0 => match v {
                1 => self.begin(),
                2 => self.stop(),
                _ => Err("unsupported flash storage transaction strobe".into()),
            },
            0xf0 => {
                if v & 1 != 0 {
                    self.state.complete = false;
                    self.flash_storage
                        .lock()
                        .unwrap()
                        .session
                        .controller_complete = false;
                }
                Ok(())
            }
            0xd0 => {
                if v & !0x101 != 0 {
                    Err("unsupported flash storage FIFO reset".into())
                } else {
                    Ok(())
                }
            }
            0xe0 => {
                cx.unmodeled();
                return;
            }
            _ => {
                self.state.regs[o as usize / 4] = v;
                if o == 0xc8 {
                    self.state.configuration_complete = true;
                }
                if o == 0x48 {
                    self.flash_storage.lock().unwrap().session.program_mode =
                        self.part.pio(v as u8) == Some(Command::Program);
                }
                if o == 0x74 {
                    self.check_aperture_read()
                } else {
                    Ok(())
                }
            }
        };
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid storage controller checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash_storage::Storage;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pio_reads_count_transferred_bytes_once_and_exclude_identity_commands() {
        let storage = Arc::new(Mutex::new(Storage::new((0..32).collect()).unwrap()));
        let mut controller = Controller::new(storage.clone(), Part::Quad20bb20);
        controller.state.regs[0] = 1;
        controller.state.regs[0x40 / 4] = 0x03;
        controller.state.regs[0x50 / 4] = 8;
        controller.state.regs[0xa0 / 4] = 3;
        controller.begin().unwrap();
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 4);
        for value in 8..12 {
            assert_eq!(controller.rx().unwrap(), value);
        }
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 4);
        assert_eq!(controller.rx().unwrap(), 12);
        assert_eq!(storage.lock().unwrap().session.reads.ranges, [(8, 13)]);
        controller.state.regs[0x40 / 4] = 0x9f;
        controller.begin().unwrap();
        assert_eq!(controller.rx().unwrap(), 0x20);
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 5);
    }

    fn mt35xu() -> (Shared, Controller) {
        let storage = Arc::new(Mutex::new(Storage::new((0..64).collect()).unwrap()));
        let controller = Controller::new(storage.clone(), Part::from_jedec(0x2c5b1b).unwrap());
        (storage, controller)
    }

    #[test]
    fn octal_part_answers_its_identity_and_four_byte_reads() {
        let (_, mut controller) = mt35xu();
        controller.state.regs[0] = 1;
        controller.state.regs[0xa0 / 4] = 2;
        controller.state.regs[0x40 / 4] = 0x9f;
        controller.begin().unwrap();
        let id: Vec<u8> = (0..3).map(|_| controller.rx().unwrap()).collect();
        assert_eq!(id, [0x2c, 0x5b, 0x1b]);
        for opcode in [0x13, 0xcc, 0xfd] {
            controller.state.regs[0x40 / 4] = opcode;
            controller.state.regs[0x50 / 4] = 16;
            controller.begin().unwrap();
            assert_eq!(controller.rx().unwrap(), 16);
        }
    }

    #[test]
    fn octal_part_refuses_commands_outside_its_set() {
        let (_, mut controller) = mt35xu();
        controller.state.regs[0] = 1;
        // Three-byte read and quad-I/O read belong to the quad part only.
        for opcode in [0x03, 0xec] {
            controller.state.regs[0x40 / 4] = opcode;
            assert!(controller.begin().unwrap_err().contains("for part 0x2c5b1b"));
        }
    }

    #[test]
    fn octal_part_erases_its_sector_size() {
        let (storage, mut controller) = mt35xu();
        controller.state.regs[0] = 1;
        controller.state.regs[0x40 / 4] = 0x06;
        controller.begin().unwrap();
        controller.state.regs[0x40 / 4] = 0xdc;
        controller.state.regs[0x50 / 4] = 0;
        controller.begin().unwrap();
        assert_eq!(storage.lock().unwrap().read_device(0, 1).unwrap(), [0xff]);
    }

    #[test]
    fn aperture_read_setup_must_match_the_part() {
        let (_, mut controller) = mt35xu();
        controller.state.regs[0x44 / 4] = 0xfd;
        controller.state.regs[0x74 / 4] = 20;
        assert!(controller.check_aperture_read().is_ok());
        controller.state.regs[0x74 / 4] = 10;
        assert!(controller.check_aperture_read().unwrap_err().contains("0xfd with 10"));
        controller.state.regs[0x44 / 4] = 0xec;
        controller.state.regs[0x74 / 4] = 10;
        assert!(controller.check_aperture_read().is_err());
        assert_eq!(Part::from_jedec(0xef4018).unwrap_err(), "unsupported serial NOR part 0xef4018");
    }
}
