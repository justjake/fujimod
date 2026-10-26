//! Two EXR II I2C channels (fff85000 / fff85040), one controller model.
//!
//! The shared engine uses start/address/byte/stop phases and physical IRQs
//! 65/66 (bus1 logical 4F). Register contracts permit relocations.
//! ACKs, register defaults and transfer latency are declared inputs. Attached
//! parts supply sensor XYZ, heartbeat, register/status, USB charger, and PWIC
//! replies; they are not SoC devices.
//!
//! I2cController connects the shared register engine to exact guest deadlines,
//! both real interrupt controllers, a shared physical cable level, and optional
//! lens-bridge register wiring. Parts commit their responses at transfer
//! completion. The frontend declares external replies; no firmware/task state
//! is an input. Strict and discovery execution policy belongs to the runtime.
pub mod construct;

use fujimod_emulator_devices::{Unsupported, WordMap};
use fujimod_emulator_parts_controls::i2c::BusParts;
use std::collections::BTreeMap;
mod device;
mod options;
pub use device::{I2cController, Wiring};
pub use options::Options;
mod controller;
mod secondary;
mod transfer;
use fujimod_emulator_devices::record::Result;
pub use transfer::{Op as Operation, RouteBank, Routes, SecondaryFinish, SecondaryRequest};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Idle,
    AwaitingAddress,
    WritingData,
    AwaitingReadAddress,
    ReadingData,
    Other(String),
}
impl Phase {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::AwaitingAddress => "awaiting-address",
            Self::WritingData => "writing-data",
            Self::AwaitingReadAddress => "awaiting-read-address",
            Self::ReadingData => "reading-data",
            Self::Other(value) => value,
        }
    }
}
impl From<String> for Phase {
    fn from(value: String) -> Self {
        match value.as_str() {
            "idle" => Self::Idle,
            "awaiting-address" => Self::AwaitingAddress,
            "writing-data" => Self::WritingData,
            "awaiting-read-address" => Self::AwaitingReadAddress,
            "reading-data" => Self::ReadingData,
            _ => Self::Other(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RxKind {
    StationaryXyz,
    TouchHeartbeat,
    PrefixedRegister,
    RegisterStatus,
    CommandRegisters,
    Other(String),
}
impl RxKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::StationaryXyz => "stationary-xyz",
            Self::TouchHeartbeat => "touch-heartbeat",
            Self::PrefixedRegister => "prefixed-register",
            Self::RegisterStatus => "register-status",
            Self::CommandRegisters => "command-registers",
            Self::Other(value) => value,
        }
    }
}
impl From<String> for RxKind {
    fn from(value: String) -> Self {
        match value.as_str() {
            "stationary-xyz" => Self::StationaryXyz,
            "touch-heartbeat" => Self::TouchHeartbeat,
            "prefixed-register" => Self::PrefixedRegister,
            "register-status" => Self::RegisterStatus,
            "command-registers" => Self::CommandRegisters,
            _ => Self::Other(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Profile {
    PrefixWrite,
    SensorWrite,
    Stationary,
    Heartbeat,
    LatchedHeartbeat,
    RegisterStatus,
    UsbDisconnected,
    Pwic61,
    PwicStartup,
    DeclaredSecondary,
    Other(String),
}
impl Profile {
    pub fn as_str(&self) -> &str {
        match self {
            Self::PrefixWrite => "endpoint9-1cxx-virtual-write-ack",
            Self::SensorWrite => "endpoint7-and9-virtual-write-ack",
            Self::Stationary => "endpoint7-stationary-z-plus-1g",
            Self::Heartbeat => "endpoint7-stationary-and9-heartbeat-counter",
            Self::LatchedHeartbeat => "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured",
            Self::RegisterStatus => "declared-register-status-pair-latched-irq-unmeasured",
            Self::UsbDisconnected => "usbcharge-disconnected-zero-registers",
            Self::Pwic61 => "usbcharge-disconnected-pwic61-zero",
            Self::PwicStartup => "usbcharge-disconnected-pwic-startup-tagged",
            Self::DeclaredSecondary => "declared-bus1-register-replies-unmeasured",
            Self::Other(value) => value,
        }
    }
}
impl From<String> for Profile {
    fn from(value: String) -> Self {
        match value.as_str() {
            "endpoint9-1cxx-virtual-write-ack" => Self::PrefixWrite,
            "endpoint7-and9-virtual-write-ack" => Self::SensorWrite,
            "endpoint7-stationary-z-plus-1g" => Self::Stationary,
            "endpoint7-stationary-and9-heartbeat-counter" => Self::Heartbeat,
            "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured" => Self::LatchedHeartbeat,
            "declared-register-status-pair-latched-irq-unmeasured" => Self::RegisterStatus,
            "usbcharge-disconnected-zero-registers" => Self::UsbDisconnected,
            "usbcharge-disconnected-pwic61-zero" => Self::Pwic61,
            "usbcharge-disconnected-pwic-startup-tagged" => Self::PwicStartup,
            "declared-bus1-register-replies-unmeasured" => Self::DeclaredSecondary,
            _ => Self::Other(value),
        }
    }
}

pub const OFFSETS: [u32; 5] = [0, 4, 8, 0x10, 0x18];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Channel {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ReceiveCommand {
    Configure,
    Nack,
    Byte(usize),
    Stop,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Registers {
    pub channel: Channel,
    pub parts: BusParts,
    pub secondary: secondary::Contract,
    pub irq: u8,
    pub transfer_ticks: u64,
    pub transfer_ticks_bool: bool,
    next_token: u64,
    pending: BTreeMap<u64, transfer::Pending>,
    post_irq: BTreeMap<u64, transfer::IrqFinish>,
    pub base: u32,
    pub modes: Vec<u32>,
    pub regs: WordMap,
    pub profile: Option<Profile>,
    pub phase: Phase,
    pub busy: bool,
    pub generation: i64,
    pub operations_started: i64,
    pub operations_completed: i64,
    pub irqs_raised: i64,
    pub unmodeled_transaction: bool,
    pub rx_bytes: Vec<u8>,
    pub rx_index: usize,
    pub rx_ready: bool,
    pub rx_configured: bool,
    pub rx_consumed: bool,
    pub receive_configured: bool,
    pub last_byte_nack: bool,
    pub received_bytes: i64,
    pub rx_byte: Option<u8>,
    pub rx_kind: Option<RxKind>,
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Unsupported(message.into()))
}

impl Registers {
    pub fn new(
        channel: Channel,
        base: u32,
        modes: Vec<u32>,
        irq: u8,
        transfer_ticks: u64,
    ) -> Self {
        Self {
            channel,
            base,
            modes,
            parts: BusParts::default(),
            secondary: secondary::Contract::default(),
            irq,
            transfer_ticks,
            transfer_ticks_bool: false,
            next_token: 0,
            pending: BTreeMap::new(),
            post_irq: BTreeMap::new(),
            regs: OFFSETS.into_iter().map(|o| (o, 0)).collect(),
            profile: None,
            phase: Phase::Idle,
            busy: false,
            generation: 0,
            operations_started: 0,
            operations_completed: 0,
            irqs_raised: 0,
            unmodeled_transaction: false,
            rx_bytes: Vec::new(),
            rx_index: 0,
            rx_ready: false,
            rx_configured: false,
            rx_consumed: false,
            receive_configured: false,
            last_byte_nack: false,
            received_bytes: 0,
            rx_byte: None,
            rx_kind: None,
        }
    }

    pub fn offset(&self, address: u64, size: u32) -> Result<u32> {
        let offset = address.checked_sub(self.base as u64);
        if size != 4
            || address & 3 != 0
            || !offset.is_some_and(|o| OFFSETS.contains(&(o as u32)) && o < 0x40)
        {
            return fail(match self.channel {
                Channel::Primary => {
                    format!("Unknown control-bus register/access {address:#x}/{size}")
                }
                Channel::Secondary => {
                    format!("Unknown secondary control-bus access {address:#x}/{size}")
                }
            });
        }
        Ok(offset.unwrap() as u32)
    }

    pub fn sensor_profile(&self) -> bool {
        matches!(
            self.profile,
            Some(Profile::Stationary | Profile::Heartbeat | Profile::LatchedHeartbeat)
        )
    }
    pub fn receive_profile(&self) -> bool {
        self.sensor_profile() || self.profile == Some(Profile::RegisterStatus)
    }

    /// The secondary channel's data read needs the live cable input first;
    /// the adapter must preflight it before calling this method.
    pub fn read(&mut self, address: u64, size: u32) -> Result<u32> {
        let offset = self.offset(address, size)?;
        if offset == 4 {
            match self.channel {
                Channel::Primary => {
                    if !self.receive_profile()
                        || self.phase != Phase::ReadingData
                        || self.busy
                        || !self.rx_ready
                    {
                        return fail("Control-bus target receive data is not modeled or not ready");
                    }
                }
                Channel::Secondary => {
                    if self.phase != Phase::ReadingData || self.busy || !self.rx_ready {
                        return fail("Bus1 target receive byte is not ready");
                    }
                    self.rx_consumed = true;
                }
            }
            self.rx_ready = false;
            self.rx_index += 1;
            self.received_bytes += 1;
            if self.channel == Channel::Secondary {
                return Ok(self.rx_byte.expect("ready bus1 receive has a byte") as u32);
            }
        }
        // The binding reports a removed word as KeyError before calling us.
        Ok(self.regs.get_or_zero(offset))
    }

    pub fn pure_read(&self, address: u64) -> Option<u32> {
        let offset = self.offset(address, 4).ok()?;
        if offset == 4 {
            None
        } else {
            self.regs.get(offset)
        }
    }

    /// Validate common write ordering; mode/enable writes are completed
    /// here. Other writes continue through the attached-part adapter.
    pub fn prepare_write(&mut self, address: u64, size: u32, value: u64) -> Result<bool> {
        let offset = self.offset(address, size)?;
        if value > u32::MAX as u64 {
            return fail(if self.channel == Channel::Primary {
                "Control-bus value is not a 32-bit word"
            } else {
                "Bus1 value exceeds word access width"
            });
        }
        let value = value as u32;
        if offset == 0x18 {
            return fail(if self.channel == Channel::Primary {
                "Control-bus status is read-only in the recovered write subset"
            } else {
                "Bus1 status is read-only in the recovered subset"
            });
        }
        if offset == 0x10 {
            if self.busy || self.phase != Phase::Idle || !self.modes.contains(&value) {
                return fail(match self.channel {
                    Channel::Secondary => {
                        "Only idle bus1 mode9, or explicit PWIC mode1, configuration is supported"
                            .into()
                    }
                    Channel::Primary => {
                        let fields = self
                            .modes
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        let comma = if self.modes.len() == 1 { "," } else { "" };
                        format!("Only idle bus configuration modes ({fields}{comma}) are supported")
                    }
                });
            }
            self.regs.insert(offset, value);
            return Ok(true);
        }
        if offset == 0 {
            match self.channel {
                Channel::Primary => {
                    if self.busy || ![0, 1].contains(&value) {
                        return fail("Unsupported control-bus enable/disable transaction");
                    }
                    if value == 1
                        && (self.phase != Phase::Idle
                            || !self.modes.contains(&self.regs.get_or_zero(0x10)))
                    {
                        return fail("Control-bus enable before supported mode configuration");
                    }
                    if value == 0 && self.phase != Phase::Idle {
                        return fail("Control-bus disable before completed stop");
                    }
                }
                Channel::Secondary => {
                    if self.busy || ![0, 1].contains(&value) || self.phase != Phase::Idle {
                        return fail("Bus1 enable/disable requires an idle completed transaction");
                    }
                    if value == 1 && !self.modes.contains(&self.regs.get_or_zero(0x10)) {
                        return fail("Bus1 enable precedes supported configuration");
                    }
                }
            }
            self.regs.insert(offset, value);
            return Ok(true);
        }
        match self.channel {
            Channel::Primary => {
                if self.regs.get_or_zero(0) & 1 == 0 {
                    return fail("Control-bus operation while disabled");
                }
                if self.busy {
                    return fail("Control-bus operation while a prior shift is pending");
                }
                if offset == 8 && value & !(if self.receive_profile() { 0x3f } else { 0x1b }) != 0 {
                    return fail("Control-bus receive/unknown command bits are unsupported");
                }
            }
            Channel::Secondary => {
                if self.regs.get_or_zero(0) & 1 == 0 || self.busy {
                    return fail("Bus1 operation while disabled or prior shift pending");
                }
            }
        }
        Ok(false)
    }

    /// ACK setup and last-byte NACK transitions do not need a target/IRQ.
    /// Byte and STOP requests are only classified here; mutations happen
    /// after the adapter's request-time IRQ/cable checks succeed.
    pub fn receive_command(&mut self, value: u32) -> Result<ReceiveCommand> {
        let length = self.rx_bytes.len();
        if self.channel == Channel::Primary {
            let expected = match self.rx_kind.as_ref() {
                Some(RxKind::StationaryXyz) => 6,
                Some(RxKind::RegisterStatus) => 2,
                Some(RxKind::TouchHeartbeat) => 1,
                Some(RxKind::PrefixedRegister) if (1..=1024).contains(&length) => length,
                Some(RxKind::CommandRegisters) if (1..=256).contains(&length) => length,
                _ => return fail("Control-bus receive has no explicit external input"),
            };
            if length != expected {
                return fail("Control-bus receive buffer differs from its bounded input class");
            }
            if self.rx_ready {
                return fail("Control-bus receive byte must be consumed before another command");
            }
            if value == 0xe && self.rx_index == 0 && self.regs.get_or_zero(8) == 0xa {
                self.regs.insert(8, value);
                self.rx_configured = true;
                return Ok(ReceiveCommand::Configure);
            }
            if value == 0xa && self.rx_index == length - 1 && self.regs.get_or_zero(8) == 0xe {
                // The final-byte NACK request clears ACK bit 2.
                self.regs.insert(8, value);
                return Ok(ReceiveCommand::Nack);
            }
            if self.rx_configured
                && value
                    == (if self.rx_index < length - 1 {
                        0x2e
                    } else {
                        0x2a
                    })
                && self.rx_index < length
                && self.regs.get_or_zero(8) == value & !0x20
            {
                return Ok(ReceiveCommand::Byte(self.rx_index));
            }
            if value == 0xb && self.rx_index == length && self.regs.get_or_zero(8) == 0xa {
                return Ok(ReceiveCommand::Stop);
            }
            return fail(format!(
                "Unsupported {} receive command {value:#x}/index{}",
                self.rx_kind.as_ref().unwrap().as_str(),
                self.rx_index
            ));
        }
        if self.rx_ready {
            return fail("Bus1 receive byte must be consumed before another command");
        }
        if value == 0xe && !self.receive_configured {
            self.receive_configured = true;
            self.regs.insert(8, value);
            return Ok(ReceiveCommand::Configure);
        }
        if value == 0xa
            && self.receive_configured
            && self.regs.get_or_zero(8) == 0xe
            && self.rx_index as i64 == length as i64 - 1
            && !self.last_byte_nack
        {
            self.last_byte_nack = true;
            self.regs.insert(8, value);
            return Ok(ReceiveCommand::Nack);
        }
        if (value == 0x2a && self.last_byte_nack && self.rx_index as i64 == length as i64 - 1)
            || (value == 0x2e
                && !self.last_byte_nack
                && self.receive_configured
                && (self.rx_index as i64) < length as i64 - 1
                && self.regs.get_or_zero(8) == 0xe)
        {
            return Ok(ReceiveCommand::Byte(self.rx_index));
        }
        if value == 0xb && self.rx_consumed && self.rx_index == length {
            return Ok(ReceiveCommand::Stop);
        }
        fail(format!("Unknown bus1 receive command {value:#x}"))
    }

    /// Called only after request-time board checks, before the start record
    /// and board.schedule. The adapter uses exactly transfer_ticks delay.
    pub fn start(&mut self) -> i64 {
        self.generation += 1;
        self.busy = true;
        self.regs.insert(0x18, 0);
        self.operations_started += 1;
        self.generation
    }

    /// Called only after completion-time checks, before endpoint effects,
    /// completion count, queue_irq and the completion record, in that order.
    pub fn complete(&mut self, status: u32, phase: Phase) {
        self.busy = false;
        self.regs.insert(0x18, status);
        self.phase = phase;
    }

    pub fn native_local_write(&self, address: u64, value: u64) -> bool {
        let Ok(offset) = self.offset(address, 4) else {
            return false;
        };
        matches!(offset, 0 | 0x10 | 0x18)
            || (offset == 8
                && ((self.phase == Phase::AwaitingAddress && value == 0xa)
                    || (self.phase == Phase::ReadingData
                        && (self.channel == Channel::Secondary || self.receive_profile())
                        && matches!(value, 0xe | 0xa))))
    }

    /// Handle only writes requiring no board or attached-part access.
    /// Returning None must have no effect: the event path repeats it.
    pub fn native_write(&mut self, address: u64, value: u64) -> Option<Result<()>> {
        let offset = self.offset(address, 4).ok()?;
        if offset == 0 || offset == 0x10 || offset == 0x18 {
            return Some(self.prepare_write(address, 4, value).map(|_| ()));
        }
        if offset != 8 {
            return None;
        }
        if self.phase == Phase::AwaitingAddress && value == 0xa {
            let result = self.prepare_write(address, 4, value);
            return Some(result.map(|_| {
                self.regs.insert(8, 0xa);
            }));
        }
        if self.phase == Phase::ReadingData
            && (self.channel == Channel::Secondary || self.receive_profile())
            && (value == 0xe || value == 0xa)
        {
            let result = self
                .prepare_write(address, 4, value)
                .and_then(|_| self.receive_command(value as u32))
                .map(|_| ());
            return Some(result);
        }
        None
    }
}

#[cfg(test)]
mod device_tests;
