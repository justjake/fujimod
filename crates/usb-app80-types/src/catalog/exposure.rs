//! Low-level adjustment status, exposure and configuration-packet values.
//!
//! These types describe the App80 handlers' argument bytes. They do not start
//! a multi-command calibration workflow or interpret device service completion
//! as a host-side polling instruction.
use super::{ArgumentKind, CommandError};

/// The one-bit value returned by the adjustment status callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureStatusOption {
    Option0,
    Option1,
    Unknown { value: u8 },
}
impl ExposureStatusOption {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Option0,
            1 => Self::Option1,
            value => Self::Unknown { value },
        }
    }
}

/// The byte written by the ready-option handler. Its callback success/failure
/// is reported separately by the firmware service transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureReadiness {
    NotReady,
    Ready,
    Unknown { value: u8 },
}
impl ExposureReadiness {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::NotReady,
            1 => Self::Ready,
            value => Self::Unknown { value },
        }
    }
}

/// First request byte: the firmware maps values 1 and 2 to modes 1 and 2 and
/// all other values to mode 0. The typed API exposes only the three choices
/// that have defined behavior in this handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureModeOption {
    Default,
    Mode1,
    Mode2,
}
impl ExposureModeOption {
    pub(super) const fn wire(self) -> u8 {
        match self {
            Self::Default => 0,
            Self::Mode1 => 1,
            Self::Mode2 => 2,
        }
    }
}

/// Fourth request byte. The handler treats exactly 1 as enabled and preserves
/// the existing mode-zero override for that value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureOption {
    Disabled,
    Enabled,
}
impl ExposureOption {
    pub(super) const fn wire(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Enabled => 1,
        }
    }
}

/// The 16-bit duration quantity read from request bytes 1 and 2. Firmware
/// multiplies this value by 100 before passing it to the adjustment routine;
/// the time unit is not established by the handler alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExposureDuration(u16);
impl ExposureDuration {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u16 {
        self.0
    }

    pub(super) const fn wire(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }
}

/// Exactly eight bytes for a kind-0 packet write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjustmentPacketPayload8([u8; 8]);
impl AdjustmentPacketPayload8 {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        data.try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    }

    pub const fn data(&self) -> &[u8; 8] {
        &self.0
    }
}

/// Exactly three bytes for a kind-2 packet write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjustmentPacketPayload3([u8; 3]);
impl AdjustmentPacketPayload3 {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        data.try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    }

    pub const fn data(&self) -> &[u8; 3] {
        &self.0
    }
}

/// Exactly two bytes for a kind-4 packet write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjustmentPacketPayload2([u8; 2]);
impl AdjustmentPacketPayload2 {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        data.try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::AdjustmentPacketLength))
    }

    pub const fn data(&self) -> &[u8; 2] {
        &self.0
    }
}

/// A kind-0..4 packet write, with a payload type that fixes the copied byte
/// count for each firmware branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentPacketWrite {
    Kind0 { payload: AdjustmentPacketPayload8 },
    Kind1 { value: u8 },
    Kind2 { payload: AdjustmentPacketPayload3 },
    Kind3 { value: u8 },
    Kind4 { payload: AdjustmentPacketPayload2 },
}
impl AdjustmentPacketWrite {
    pub(super) const fn kind(self) -> u8 {
        match self {
            Self::Kind0 { .. } => 0,
            Self::Kind1 { .. } => 1,
            Self::Kind2 { .. } => 2,
            Self::Kind3 { .. } => 3,
            Self::Kind4 { .. } => 4,
        }
    }

    pub(super) const fn payload(self) -> ([u8; 8], usize) {
        let mut bytes = [0; 8];
        match self {
            Self::Kind0 { payload } => (*payload.data(), 8),
            Self::Kind1 { value } => {
                bytes[0] = value;
                (bytes, 1)
            }
            Self::Kind2 { payload } => {
                bytes[0] = payload.data()[0];
                bytes[1] = payload.data()[1];
                bytes[2] = payload.data()[2];
                (bytes, 3)
            }
            Self::Kind3 { value } => {
                bytes[0] = value;
                (bytes, 1)
            }
            Self::Kind4 { payload } => {
                bytes[0] = payload.data()[0];
                bytes[1] = payload.data()[1];
                (bytes, 2)
            }
        }
    }
}

/// A kind-0x20..0x23 packet read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentPacketRead {
    Kind20,
    Kind21,
    Kind22,
    Kind23,
}
impl AdjustmentPacketRead {
    pub(super) const fn kind(self) -> u8 {
        match self {
            Self::Kind20 => 0x20,
            Self::Kind21 => 0x21,
            Self::Kind22 => 0x22,
            Self::Kind23 => 0x23,
        }
    }
}

/// One direct packet operation. Higher-level adjustment sequences belong in
/// an operations module and must remain explicit at the call site.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentPacketAction {
    Write { packet: AdjustmentPacketWrite },
    Read { kind: AdjustmentPacketRead },
}

/// Bytes copied from a packet-read reply into App80 argument slots by the
/// handler. For kind 0x20 the handler requests nine packet bytes but publishes
/// only bytes 1 through 7; its eighth data byte is not copied to App80 output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentPacketReadData {
    Kind20 { data: [u8; 7] },
    Kind21 { value: u8 },
    Kind22 { data: [u8; 3] },
    Kind23 { value: u8 },
}

#[cfg(test)]
#[path = "exposure_tests.rs"]
mod tests;
