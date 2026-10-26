//! Peripheral service argument layouts. Port numbers are protocol choices,
//! not USB buses or promises about physical devices on an unidentified body.
use super::{ArgumentKind, CommandError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterAction {
    AudioValue { value: u8 },
    DisplayFirst { selector: u8, value: u16 },
    DisplayStub { selector: u8, value: u16 },
    DisplaySecond { selector: u8, value: u16 },
    PowerWrite { register: u16, value: u8 },
    PositionPair { first: u16, second: u16 },
    NoopFirst,
    NoopSecond,
}
impl RegisterAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::AudioValue { value } => {
                put(0, 0);
                put(4, value);
            }
            Self::DisplayFirst { selector, value }
            | Self::DisplayStub { selector, value }
            | Self::DisplaySecond { selector, value } => {
                put(
                    0,
                    match self {
                        Self::DisplayFirst { .. } => 1,
                        Self::DisplayStub { .. } => 2,
                        _ => 3,
                    },
                );
                put(2, selector);
                let [high, low] = value.to_be_bytes();
                put(3, high);
                put(4, low);
            }
            Self::PowerWrite { register, value } => {
                put(0, 4);
                let [high, low] = register.to_be_bytes();
                put(1, high);
                put(2, low);
                put(4, value);
            }
            Self::PositionPair { first, second } => {
                put(0, 5);
                for (index, byte) in first
                    .to_be_bytes()
                    .into_iter()
                    .chain(second.to_be_bytes())
                    .enumerate()
                {
                    put(index + 1, byte);
                }
            }
            Self::NoopFirst => put(0, 7),
            Self::NoopSecond => put(0, 8),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cLength(u8);
impl I2cLength {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if (1..=10).contains(&value) {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::I2cLength))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cPayload {
    length: I2cLength,
    bytes: [u8; 10],
}
impl I2cPayload {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        let length = u8::try_from(data.len())
            .map_err(|_| CommandError::Argument(ArgumentKind::I2cLength))?;
        let length = I2cLength::new(length)?;
        let mut bytes = [0; 10];
        bytes[..data.len()].copy_from_slice(data);
        Ok(Self { length, bytes })
    }
    pub fn data(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length.value())]
    }
}
/// Port 3 has a different contract and is represented by WriteSingle only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cPort {
    Port0,
    Port1,
    Port2,
    Port4,
    Port5,
}
impl I2cPort {
    fn wire(self) -> u8 {
        match self {
            Self::Port0 => 0,
            Self::Port1 => 1,
            Self::Port2 => 2,
            Self::Port4 => 4,
            Self::Port5 => 5,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cAction {
    Read {
        port: I2cPort,
        command: u8,
        length: I2cLength,
    },
    Write {
        port: I2cPort,
        command: u8,
        payload: I2cPayload,
    },
    WriteSingle {
        command: u8,
        value: u8,
    },
}
impl I2cAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        let (mode, port, command, length) = match self {
            Self::Read {
                port,
                command,
                length,
            } => (0, port.wire(), command, length.value()),
            Self::Write {
                port,
                command,
                payload,
            } => {
                for (index, byte) in payload.bytes.into_iter().enumerate() {
                    put(4 + index, byte);
                }
                (1, port.wire(), command, payload.length.value())
            }
            Self::WriteSingle { command, value } => {
                put(4, value);
                (1, 3, command, 1)
            }
        };
        for (index, byte) in [mode, port, length, command].into_iter().enumerate() {
            put(index, byte);
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccumulatorAction {
    Start,
    StopAndRead,
}
impl AccumulatorAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        put(
            0,
            match self {
                Self::Start => 1,
                Self::StopAndRead => 0,
            },
        );
    }
}

/// Direct service actions; neither returns a charge or battery measurement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatteryAction {
    ResetPending,
    CallbackWait,
}
impl BatteryAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        put(
            0,
            match self {
                Self::ResetPending => 0,
                Self::CallbackWait => 1,
            },
        );
        // Firmware reads this byte on reset; the reviewed lower helper ignores it.
        put(1, 0);
    }
}
/// Solid inputs describe the packed fields, not inferred RGB/YUV channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayPatternAction {
    ResetPreset,
    Bars,
    Solid { repeated: u8, middle: u8, low: u8 },
}
impl DisplayPatternAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::ResetPreset => put(0, 0),
            Self::Bars => put(0, 1),
            Self::Solid {
                repeated,
                middle,
                low,
            } => {
                put(0, 2);
                // Slot 1 is unused. Firmware packs repeated twice in the high half.
                put(2, repeated);
                put(3, low);
                put(4, middle);
            }
        }
    }
}
