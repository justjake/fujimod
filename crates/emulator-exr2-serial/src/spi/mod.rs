//! SPI controller channels FFF84000/FFF84040 expose FIFO levels, receive
//! consumption, and low-half W1C status. SPI0 uses an explicit fixed-low MISO input.
//! GPIO selection and endpoint registers belong to attached parts/runtime;
//! no lens, power-management, flash or panel effects are inferred here.
pub mod byte_fifo;
pub mod channel;

use fujimod_emulator_devices::record::{Result, Unsupported};
use fujimod_emulator_devices::{Action, Record, Value, WordMap};
pub const BASE0: u32 = 0xfff84000;
pub const BASE1: u32 = 0xfff84040;
/// Known electrical profiles are decoded once at the binding boundary.
#[derive(Clone, Debug, Default)]
pub enum InputProfile {
    #[default]
    Unresolved,
    AbsentHotShoeFlash,
    Other(String),
}
impl InputProfile {
    pub fn from_host(value: Option<String>) -> Self {
        match value {
            None => Self::Unresolved,
            Some(value) if value == "absent-hot-shoe-flash-fixed-low-miso" => {
                Self::AbsentHotShoeFlash
            }
            Some(value) => Self::Other(value),
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Unresolved => None,
            Self::AbsentHotShoeFlash => Some("absent-hot-shoe-flash-fixed-low-miso"),
            Self::Other(value) => Some(value),
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct ShiftRegisters {
    pub busy: bool,
    pub ready: bool,
    pub reply: u32,
    pub generation: u64,
    pub input_profile: InputProfile,
    pub rx: Option<u32>,
    pub started: u64,
    pub completed: u64,
    pub consumed: u64,
}
impl ShiftRegisters {
    pub fn read(
        &mut self,
        channel: u8,
        cpu: i64,
        offset: u32,
        mut value: u32,
        actions: &mut Vec<Action>,
    ) -> Result<u32> {
        if channel == 0 {
            match offset {
                0 => value = (value & !1) | self.busy as u32,
                0x18 => {
                    value = self.rx.ok_or_else(|| {
                        Unsupported::from("SPI0 receive data has no completed modeled exchange")
                    })?;
                    self.rx = None;
                    self.consumed += 1;
                    actions.push(Action::Log(Record::new(
                        "SPI0 receive consumed",
                        [
                            ("cpu", Value::int(cpu)),
                            ("value", Value::hex(value as u128)),
                            (
                                "profile",
                                self.input_profile.as_str().map_or(Value::None, Value::str),
                            ),
                        ],
                    )));
                }
                0x10 | 0x14 => {
                    value = (value & !0x1f) | (offset == 0x10 && self.rx.is_some()) as u32
                }
                _ => (),
            }
        } else {
            match offset {
                0xc => value = (value & !0x300) | if self.ready { 0x300 } else { 0 },
                0x10 => value = (value & !0x1f) | self.ready as u32,
                0x18 => {
                    if !self.ready {
                        return Err(Unsupported::from("SPI response read before completion"));
                    }
                    value = self.reply;
                    self.ready = false;
                }
                _ => (),
            }
        }
        Ok(value)
    }
    pub fn write(
        &mut self,
        channel: u8,
        cpu: i64,
        offset: u32,
        value: u32,
        registers: &mut WordMap,
        actions: &mut Vec<Action>,
    ) -> Result<bool> {
        let base = if channel == 0 { BASE0 } else { BASE1 };
        if offset == 0x18 {
            return Err(Unsupported::from(if channel == 0 {
                "SPI0 receive data is read-only"
            } else {
                "SPI receive data is read-only"
            }));
        }
        if channel == 0 && offset == 0x1c {
            return Ok(true);
        }
        if offset == 0xc {
            // Acknowledgments OR low bits into the word, clearing flags rather
            // than setting errors. Keep the retained high half.
            let old = registers.get_or_zero(base + offset);
            registers.insert(base + offset, old & !(value & 0xffff));
        } else {
            registers.insert(base + offset, value);
        }
        if channel == 0 && offset == 8 && value & 0x10000000 != 0 {
            self.generation += 1;
            self.busy = false;
            self.rx = None;
            registers.insert(base + 0xc, registers.get_or_zero(base + 0xc) & !0x300);
            actions.push(Action::Log(Record::new(
                "SPI0 FIFO reset with empty queues",
                [
                    ("cpu", Value::int(cpu)),
                    (
                        "provenance",
                        Value::str("FIFO reset configuration; cancels pending modeled shift"),
                    ),
                ],
            )));
        }
        Ok(offset == 0x1c)
    }
    pub fn preflight(&self, command: u32, control: u32) -> Result<()> {
        if self.busy || control & 0x80 == 0 {
            return Err(Unsupported::from("SPI start while busy or disabled"));
        }
        if command > 0xffff {
            return Err(Unsupported(format!(
                "Unknown auxiliary MCU SPI request {command:#x}"
            )));
        }
        Ok(())
    }
    pub fn preflight0(
        &self,
        value: u32,
        configuration: (u32, u32, u32),
        required: (u32, u32, u32),
        status: u32,
    ) -> Result<()> {
        if matches!(self.input_profile, InputProfile::Unresolved) {
            return Err(Unsupported(format!(
                "Unmodeled SPI0 exchange {value:#x}; endpoint protocol unresolved"
            )));
        }
        if self.busy || self.rx.is_some() {
            return Err(Unsupported::from(
                "SPI0 shift while busy or receive FIFO unread",
            ));
        }
        if configuration != required || status & 0xffff0000 != 0 || value > 0xff {
            return Err(Unsupported(format!(
                "Unsupported SPI0 byte transport configuration ({}, {}, {}), TX={value:#x}",
                configuration.0, configuration.1, configuration.2
            )));
        }
        Ok(())
    }
    /// Start only after the runtime has validated the attached endpoint.
    pub fn start(&mut self, channel: u8) -> u64 {
        self.generation += 1;
        self.busy = true;
        if channel == 0 {
            self.started += 1;
        } else {
            self.ready = false;
        }
        self.generation
    }
    pub fn complete(
        &mut self,
        channel: u8,
        generation: u64,
        reply: u32,
        registers: &mut WordMap,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.busy = false;
        if channel == 0 {
            self.rx = Some(reply);
            self.completed += 1;
            registers.insert(BASE0 + 0xc, registers.get_or_zero(BASE0 + 0xc) | 0x300);
        } else {
            self.reply = reply;
            self.ready = true;
        }
        true
    }
}

use self::channel::SerialChannel;
use fujimod_emulator_devices::{Cx, Device};

/// One physical controller channel with multiple GPIO-selected attached parts.
/// Register writes are shared bus configuration; only one route can own the
/// shifter or unread RX words. Parts keep their independent wire state.
pub struct SpiController {
    channels: Vec<SerialChannel>,
    active: usize,
    base: u64,
}
impl SpiController {
    pub fn status_value(&self, route: usize, register: u8) -> std::result::Result<u8, String> {
        self.channels
            .get(route)
            .ok_or("serial status input route is not attached")?
            .status_value(register)
    }
    pub fn status_input(
        &mut self,
        route: usize,
        register: u8,
        value: u8,
    ) -> std::result::Result<(), String> {
        self.channels
            .get_mut(route)
            .ok_or("serial status input route is not attached")?
            .status_input(register, value)
    }
    pub fn new(channels: Vec<SerialChannel>) -> std::result::Result<Self, String> {
        let base = channels
            .first()
            .ok_or("SPI controller has no attached routes")?
            .options
            .base;
        if channels.len() > 16 || channels.iter().any(|c| c.options.base != base) {
            return Err("SPI routes require one controller register window".into());
        }
        Ok(Self {
            channels,
            active: 0,
            base,
        })
    }
    fn selected(&self) -> std::result::Result<usize, String> {
        let mut selected = None;
        for (i, c) in self.channels.iter().enumerate() {
            if c.route().is_ok() && c.selected()? {
                if selected.is_some() {
                    return Err("ambiguous SPI chip selection".into());
                }
                selected = Some(i);
            }
        }
        selected.ok_or_else(|| {
            "no attached SPI route admits the programmed transport and chip select".into()
        })
    }
}
impl Device for SpiController {
    fn serial_status(&self) -> Option<&(dyn fujimod_emulator_devices::capability::SerialStatus + 'static)> {
        Some(self)
    }
    fn serial_status_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::SerialStatus + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> std::result::Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(
            fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.active)?.with_children(
                self.channels
                    .iter()
                    .map(Device::checkpoint)
                    .collect::<std::result::Result<_, _>>()?,
            ),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> std::result::Result<(), String> {
        let active: usize = saved.decode(self.name())?;
        if active >= self.channels.len() || saved.children().len() != self.channels.len() {
            return Err("SPI snapshot route count differs".into());
        }
        for (channel, saved) in self.channels.iter().zip(saved.children()) {
            channel.validate_checkpoint(saved)?;
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> std::result::Result<(), String> {
        self.validate_checkpoint(saved)?;
        for (channel, saved) in self.channels.iter_mut().zip(saved.children()) {
            channel.restore_checkpoint(saved)?;
        }
        self.active = saved.decode(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"active":self.active,"channels":self.channels.iter().map(|c|c.fingerprint()).collect::<std::result::Result<Vec<_>,_>>()?}),
        )
    }
    fn name(&self) -> &'static str {
        "spi-controller"
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"active":self.active,"channels":self.channels.iter().map(|c|c.save()).collect::<Vec<_>>()})
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        if cx.refusal().is_some() {
            return 0;
        }
        self.channels[self.active].read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        if address == self.base + 0x1c && size == 4 {
            let selected = match self.selected() {
                Ok(i) => i,
                Err(e) => {
                    cx.fail(e);
                    return;
                }
            };
            if selected != self.active
                && (self.channels[self.active].pending.is_some()
                    || !self.channels[self.active].rx.is_empty())
            {
                cx.fail("SPI route changed before the shared shifter and RX FIFO drained");
                return;
            }
            self.active = selected;
            self.channels[selected].write(cx, address, size, value);
        } else {
            for channel in &mut self.channels {
                channel.write(cx, address, size, value);
                if cx.refusal().is_some() {
                    return;
                }
            }
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for channel in &mut self.channels {
            channel.advance(cx);
            if cx.refusal().is_some() {
                return;
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.channels
            .iter()
            .filter_map(|c| c.next_deadline(now))
            .min()
    }
}

impl fujimod_emulator_devices::capability::SerialStatus for SpiController {
    fn status_value(&self, route: usize, register: u8) -> std::result::Result<u8, String> {
        SpiController::status_value(self, route, register)
    }
    fn status_input(&mut self, route: usize, register: u8, value: u8) -> std::result::Result<(), String> {
        SpiController::status_input(self, route, register, value)
    }
}
