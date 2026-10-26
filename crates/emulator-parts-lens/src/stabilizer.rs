//! Four-byte MSB-first stabilization protocol, with five-byte F0..F6
//! configuration packets ending in A0. Chip select may pulse between bytes.
//! Status, telemetry, and command widths are declared part data. Opaque
//! transmit-only uploads have no effect on status or command completion.
//! No optical or uploaded-program execution model is supplied.
use fujimod_emulator_parts_io::spi::{Endpoint, Shift};
use fujimod_emulator_devices::Cx;
use serde::{Deserialize, Serialize};

const RAM_WORDS: usize = 65536;
const SLOTS: usize = RAM_WORDS * 2;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Write,
    Select,
    RamWrite { bank: u8 },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub code: u8,
    pub bits: u8,
    pub kind: CommandKind,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub status: u8,
    pub telemetry: u32,
    pub commands: Vec<Command>,
}

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Slot {
    used: bool,
    bank: u8,
    address: u32,
    data: u32,
}

pub struct Stabilizer {
    status: u8,
    telemetry: u32,
    commands: [Option<Command>; 256],
    bytes: [u8; 5],
    length: usize,
    selector: Option<u8>,
    address: u32,
    raw_address: bool,
    registers: [u32; 256],
    configuration: [u32; 7],
    memory: Box<[Slot]>,
    used: usize,
}
impl Stabilizer {
    pub fn new(options: Options, _wiring: ()) -> Result<Self, String> {
        let mut commands = [None; 256];
        for command in options.commands {
            if command.code == 0
                || command.code >= 0xf0
                || command.bits > 24
                || commands[command.code as usize].is_some()
                || matches!(command.kind, CommandKind::RamWrite { bank } if !matches!(bank, 2 | 0x20))
            {
                return Err("invalid or duplicate stabilizer command declaration".into());
            }
            commands[command.code as usize] = Some(command);
        }
        Ok(Self {
            status: options.status,
            telemetry: options.telemetry,
            commands,
            bytes: [0; 5],
            length: 0,
            selector: None,
            address: 0,
            raw_address: false,
            registers: [0; 256],
            configuration: [0; 7],
            memory: vec![Slot::default(); SLOTS].into_boxed_slice(),
            used: 0,
        })
    }
    fn slot(&self, bank: u8, address: u32) -> usize {
        let mut index = (address.wrapping_mul(0x9e3779b9)
            ^ u32::from(bank).wrapping_mul(0x85ebca6b)) as usize
            & (SLOTS - 1);
        loop {
            let slot = self.memory[index];
            if !slot.used || slot.bank == bank && slot.address == address {
                return index;
            }
            index = (index + 1) & (SLOTS - 1);
        }
    }
    fn reply(&self) -> u32 {
        match self.selector {
            Some(8) => u32::from(self.status),
            Some(0xe) => self.telemetry,
            Some(bank) => {
                let slot = self.memory[self.slot(bank, self.address)];
                if slot.used { slot.data } else { 0 }
            }
            None => 0,
        }
    }
    fn validate_word(&self, word: u32) -> Result<(), String> {
        if self.raw_address {
            return Ok(());
        }
        let code = (word >> 24) as u8;
        let data = word & 0xffffff;
        if code == 0 {
            return if data == 0 && self.selector.is_some() {
                Ok(())
            } else {
                Err("stabilizer read requires a selector and zero dummy word".into())
            };
        }
        let command = self.commands[code as usize].ok_or("unsupported stabilizer command")?;
        if data >= 1 << command.bits {
            return Err("stabilizer command data exceeds its declared width".into());
        }
        if let CommandKind::RamWrite { bank } = command.kind {
            if self.selector != Some(bank) {
                return Err("stabilizer RAM write requires its address selector".into());
            }
            if self.used == RAM_WORDS && !self.memory[self.slot(bank, self.address)].used {
                return Err("stabilizer RAM exceeds its declared bound".into());
            }
        }
        Ok(())
    }
    pub fn register(&self, code: u8) -> u32 {
        self.registers[code as usize]
    }
    pub fn configuration(&self, code: u8) -> Option<u32> {
        (0xf0..=0xf6)
            .contains(&code)
            .then(|| self.configuration[(code - 0xf0) as usize])
    }
}
impl Endpoint for Stabilizer {
    fujimod_emulator_devices::checkpoint_fields!(status:u8,telemetry:u32,#[serde(with="fujimod_emulator_devices::checkpoint::array")] commands:[Option<Command>;256],bytes:[u8;5],length:usize,selector:Option<u8>,address:u32,raw_address:bool,#[serde(with="fujimod_emulator_devices::checkpoint::array")] registers:[u32;256],configuration:[u32;7],memory:Box<[Slot]>,used:usize);

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        let mut memory: Vec<_> = self
            .memory
            .iter()
            .filter(|s| s.used)
            .map(|s| (s.bank, s.address, s.data))
            .collect();
        memory.sort_unstable();
        Ok(
            serde_json::json!({"status":self.status,"telemetry":self.telemetry,"commands":self.commands.as_slice(),
            "partial":self.bytes,"length":self.length,"selector":self.selector,"address":self.address,
            "raw_address":self.raw_address,"registers":self.registers.as_slice(),"configuration":self.configuration,"memory":memory}),
        )
    }
    fn state(&self) -> serde_json::Value {
        serde_json::json!({"partial_bytes":&self.bytes[..self.length],"selector":self.selector,"address":self.address,
            "registers":self.registers.as_slice(),"configuration":self.configuration,"memory_words":self.used})
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        if shift.bits != 8 || shift.word > 255 {
            return Err("stabilizer requires byte frames".into());
        }
        if !shift.receive {
            return if self.length == 0 {
                Ok(())
            } else {
                Err("stabilizer transport changed during a register word".into())
            };
        }
        let value = shift.word as u8;
        let first = if self.length == 0 {
            value
        } else {
            self.bytes[0]
        };
        if (0xf0..=0xf6).contains(&first) && !self.raw_address {
            if self.length == 4 && value != 0xa0 {
                return Err("stabilizer configuration requires its A0 terminator".into());
            }
        } else if self.length == 3 {
            self.validate_word(u32::from_be_bytes([
                self.bytes[0],
                self.bytes[1],
                self.bytes[2],
                value,
            ]))?;
        }
        Ok(())
    }
    fn transfer(&mut self, _: &mut Cx<'_>, shift: Shift, _: u64) -> Result<u16, String> {
        self.validate(shift)?;
        if !shift.receive {
            return Ok(0);
        }
        let value = shift.word as u8;
        let first = if self.length == 0 {
            value
        } else {
            self.bytes[0]
        };
        if (0xf0..=0xf6).contains(&first) && !self.raw_address {
            self.bytes[self.length] = value;
            self.length += 1;
            if self.length == 5 {
                self.configuration[(first - 0xf0) as usize] =
                    u32::from_le_bytes([self.bytes[1], self.bytes[2], self.bytes[3], 0]);
                self.length = 0;
            }
            return Ok(0);
        }
        let response = if first == 0 && !self.raw_address {
            (self.reply() >> (24 - self.length * 8)) as u8
        } else {
            0
        };
        self.bytes[self.length] = value;
        self.length += 1;
        if self.length < 4 {
            return Ok(u16::from(response));
        }
        let word = u32::from_be_bytes(self.bytes[..4].try_into().unwrap());
        self.length = 0;
        if self.raw_address {
            self.address = word;
            self.raw_address = false;
            return Ok(u16::from(response));
        }
        let code = (word >> 24) as u8;
        let data = word & 0xffffff;
        if code == 0 {
            if matches!(self.selector, Some(2 | 0x18 | 0x20)) {
                self.address = self.address.wrapping_add(1);
            }
        } else {
            let command = self.commands[code as usize].unwrap();
            match command.kind {
                CommandKind::Select => {
                    self.selector = Some(code);
                    self.address = data;
                    self.raw_address = code == 0x20;
                }
                CommandKind::Write => self.registers[code as usize] = data,
                CommandKind::RamWrite { bank } => {
                    self.registers[code as usize] = data;
                    let index = self.slot(bank, self.address);
                    self.used += usize::from(!self.memory[index].used);
                    self.memory[index] = Slot {
                        used: true,
                        bank,
                        address: self.address,
                        data,
                    };
                    self.address = self.address.wrapping_add(1);
                }
            }
        }
        Ok(u16::from(response))
    }
}
