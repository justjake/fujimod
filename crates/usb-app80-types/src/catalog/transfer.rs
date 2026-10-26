//! Direct service-transfer commands. These are firmware requests, not USB data
//! upload/download compositions or assertions that a hardware callee is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferOperation {
    Operation0,
    Operation1,
    Operation2,
    Operation3,
    Operation6,
}
impl TransferOperation {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Operation0 => 0,
            Self::Operation1 => 1,
            Self::Operation2 => 2,
            Self::Operation3 => 3,
            Self::Operation6 => 255,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferChannel {
    Operation1,
    Operation3,
    Operation4,
}
impl TransferChannel {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Operation1 => 0,
            Self::Operation3 => 1,
            Self::Operation4 => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferPrepareResult {
    Success,
    PreparationError,
    Status7,
    Unknown(u8),
}
impl TransferPrepareResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            2 => Self::PreparationError,
            3 => Self::Status7,
            n => Self::Unknown(n),
        }
    }
}
/// The operation selector's callback-error path can leave its output unchanged.
/// Unknown bytes, including the initial 255, never request a host poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferResult {
    Success,
    Failed,
    Unknown(u8),
}
impl TransferResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::Failed,
            n => Self::Unknown(n),
        }
    }
}

use super::{ArgumentKind, CommandError};
/// The transfer engine's aligned 64 KiB word space, distinct from CPU RAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferWordAddress(u32);
impl TransferWordAddress {
    pub fn new(value: u32) -> Result<Self, CommandError> {
        if value < 0x10000 && value.is_multiple_of(4) {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::TransferWordAddress))
        }
    }
    pub fn value(self) -> u32 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferPairFirst(u16);
impl TransferPairFirst {
    pub fn new(value: u16) -> Result<Self, CommandError> {
        if value < 512 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::TransferPairFirst))
        }
    }
    pub fn value(self) -> u16 {
        self.0
    }
}
/// All 32 address bits are forwarded; the service handler adds no alignment or
/// aperture check. This type does not claim that the target implements the read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferHalfwordAddress(u32);
impl TransferHalfwordAddress {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub fn value(self) -> u32 {
        self.0
    }
}
/// Opaque request data forwarded to the transfer helper, not a host disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferPayloadByte(u8);
impl TransferPayloadByte {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferWordReadResult {
    Value { raw_value: u32 },
    Rejected,
    Unknown { raw_status: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferPairResult {
    Values { first: u8, second: u8 },
    CallbackError,
    Rejected,
    WaitError,
    Unknown { raw_status: u8 },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferOption {
    Option0,
    Option1,
    Option2,
    Option3,
    Option4,
    Option5,
    Option6,
    Option7,
}
impl TransferOption {
    pub(super) fn wire(self) -> (u8, u8) {
        match self {
            Self::Option0 => (0, 0),
            Self::Option1 => (1, 0),
            Self::Option2 => (2, 0),
            Self::Option3 => (3, 0),
            Self::Option4 => (0, 1),
            Self::Option5 => (1, 1),
            Self::Option6 => (2, 1),
            Self::Option7 => (3, 1),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferSelectedWordKind {
    Kind9,
    Kind10,
    Kind11,
    Kind12,
    Kind13,
    Kind14,
}
impl TransferSelectedWordKind {
    pub(super) fn wire(self) -> (u8, u8) {
        match self {
            Self::Kind9 => (2, 1),
            Self::Kind10 => (2, 0),
            Self::Kind11 => (1, 1),
            Self::Kind12 => (1, 0),
            Self::Kind13 => (0, 1),
            Self::Kind14 => (0, 0),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferWord7Kind {
    Kind7,
    Kind8,
}
impl TransferWord7Kind {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Kind7 => 1,
            Self::Kind8 => 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferWord16Kind {
    Kind16,
    Kind17,
}
impl TransferWord16Kind {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Kind16 => 1,
            Self::Kind17 => 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferLoadMode {
    Setup,
    Run,
    Request3One,
    Request3Zero,
}
impl TransferLoadMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Setup => 0,
            Self::Run => 1,
            Self::Request3One => 3,
            Self::Request3Zero => 4,
        }
    }
}
