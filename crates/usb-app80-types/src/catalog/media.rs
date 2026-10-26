//! Reviewed media command layouts. These encode service requests, not host
//! playback workflows or claims that a firmware's hardware callees are active.
use super::{ArgumentKind, CommandError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Toggle {
    Disabled,
    Enabled,
}
impl Toggle {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::Enabled => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayOptionAction {
    IdlePins,
    ForwardPair { first: u8, second: u8 },
}
impl DisplayOptionAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        put(0, 0);
        match self {
            Self::IdlePins => put(1, 0),
            Self::ForwardPair { first, second } => {
                put(1, 1);
                put(2, first);
                put(3, second);
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayMode {
    Option0,
    Option1,
}
impl DisplayMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Option0 => 0,
            Self::Option1 => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoundClip(u8);
impl SoundClip {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value < 23 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::SoundClip))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundFormat {
    Format1,
    Format2,
    Format3,
}
impl SoundFormat {
    fn wire(self) -> u8 {
        match self {
            Self::Format1 => 0,
            Self::Format2 => 1,
            Self::Format3 => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundTrack {
    Track2,
    Track3,
    Track4,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundOperation {
    First,
    Second,
    Third,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoundAction {
    Clip {
        clip: SoundClip,
        format: SoundFormat,
    },
    Track {
        track: SoundTrack,
        operation: SoundOperation,
        format: SoundFormat,
    },
    Noop,
}
impl SoundAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        let (clip, format) = match self {
            Self::Clip { clip, format } => (clip.value(), format),
            Self::Track {
                track,
                operation,
                format,
            } => {
                let base = match track {
                    SoundTrack::Track2 => 23,
                    SoundTrack::Track3 => 26,
                    SoundTrack::Track4 => 29,
                };
                let offset = match operation {
                    SoundOperation::First => 0,
                    SoundOperation::Second => 1,
                    SoundOperation::Third => 2,
                };
                (base + offset, format)
            }
            Self::Noop => {
                put(0, 0);
                return;
            }
        };
        put(0, 1);
        put(1, clip);
        put(2, format.wire());
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HdmiValue(u16);
impl HdmiValue {
    pub fn new(value: u16) -> Result<Self, CommandError> {
        if value < 4096 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::HdmiValue))
        }
    }
    pub fn value(self) -> u16 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HdmiPacket([u8; 16]);
impl HdmiPacket {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        data.try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::HdmiPacketLength))
    }
    pub fn data(&self) -> &[u8; 16] {
        &self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HdmiAction {
    ResetStub,
    Packet {
        packet: HdmiPacket,
    },
    Triplet {
        first: HdmiValue,
        second: HdmiValue,
        third: HdmiValue,
    },
}
impl HdmiAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::ResetStub => {
                put(0, 0);
                put(1, 0);
                put(2, 0);
            }
            Self::Packet { packet } => {
                put(0, 0);
                put(1, 1);
                put(2, 0);
                for (index, byte) in packet.0.into_iter().rev().enumerate() {
                    put(3 + index, byte);
                }
            }
            Self::Triplet {
                first,
                second,
                third,
            } => {
                put(0, 1);
                for (index, byte) in [first, second, third]
                    .into_iter()
                    .flat_map(|value| value.value().to_be_bytes())
                    .enumerate()
                {
                    put(3 + index, byte);
                }
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeAction {
    Control { enabled: Toggle },
    EnableAndSetFlag { enabled: Toggle },
}
impl ChargeAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        let (operation, value) = match self {
            Self::Control { enabled: value } => (0, value),
            Self::EnableAndSetFlag { enabled: value } => (1, value),
        };
        put(0, operation);
        put(1, value.wire());
        put(2, 255);
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeQuery {
    Presence,
    State,
}
impl ChargeQuery {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        put(
            0,
            match self {
                Self::Presence => 0,
                Self::State => 1,
            },
        );
        put(1, 255);
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargeResult {
    Success,
    Failed,
    Unknown(u8),
}
impl ChargeResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::Failed,
            other => Self::Unknown(other),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChargePresence {
    Failed,
    Absent,
    Present,
    Unknown(u8),
}
impl ChargePresence {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Failed,
            1 => Self::Absent,
            2 => Self::Present,
            other => Self::Unknown(other),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportedChargeState {
    Failed,
    Value0,
    Values2To5,
    Value6,
    OtherValue,
    Unknown(u8),
}
impl ReportedChargeState {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Failed,
            3 => Self::Value0,
            4 => Self::Values2To5,
            5 => Self::Value6,
            6 => Self::OtherValue,
            other => Self::Unknown(other),
        }
    }
}

/// Number of firmware audio chunks (0..=10), not a host USB transfer size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioCount(u8);
impl AudioCount {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value <= 10 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::AudioCount))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
/// AV-output request used before capture; physical routing is firmware-specific.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioRoute {
    Request82,
    Request81,
}
impl AudioRoute {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Request82 => 0,
            Self::Request81 => 1,
        }
    }
}
/// Playback copies the firmware capture buffer, optionally zeroing one interleaved lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioPlayback {
    AllLanes,
    ZeroSecondLane,
    ZeroFirstLane,
}
impl AudioPlayback {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::AllLanes => 0,
            Self::ZeroSecondLane => 1,
            Self::ZeroFirstLane => 2,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioResult {
    Success,
    Failed,
    Unknown(u8),
}
impl AudioResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::Failed,
            n => Self::Unknown(n),
        }
    }
}
/// Each variant invokes one firmware command. Region kinds are firmware terms.
/// Kinds 1 clear diagnostic records; kinds 0/2 clear service diagnostics and reset profiles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageErase {
    Kind1Regions,
    Kind0And2Regions,
    Kind0And1And2Regions,
    AuxiliaryRegion,
    ResetProfiles,
}
impl StorageErase {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Kind1Regions => 1,
            Self::Kind0And2Regions => 2,
            Self::Kind0And1And2Regions => 3,
            Self::AuxiliaryRegion => 4,
            Self::ResetProfiles => 5,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageEraseResult {
    Success,
    Failed,
    Unknown(u8),
}
impl StorageEraseResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::Failed,
            n => Self::Unknown(n),
        }
    }
}
/// Encoded numbering-snapshot classification. Zero is shared by success/error
/// paths; this byte alone cannot establish success or physical storage state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageStatus {
    Zero,
    Value4,
    Value8,
    Value16,
    Value32,
    Unknown(u8),
}
impl StorageStatus {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Zero,
            4 => Self::Value4,
            8 => Self::Value8,
            16 => Self::Value16,
            32 => Self::Value32,
            n => Self::Unknown(n),
        }
    }
}

/// Include the NAND spare bytes in the firmware's selected storage buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NandSpare {
    Omit,
    Include,
}
impl NandSpare {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Omit => 0,
            Self::Include => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NandReadResult {
    Success,
    RejectedOrWaitError,
    OtherError,
    Unknown(u8),
}
impl NandReadResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::RejectedOrWaitError,
            2 => Self::OtherError,
            n => Self::Unknown(n),
        }
    }
}
