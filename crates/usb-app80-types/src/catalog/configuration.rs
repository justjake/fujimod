//! Direct firmware configuration requests. Read requests can first change flags;
//! returned bytes are raw snapshots, not verified settings or memory values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationBank {
    Bank0,
    Bank1,
}
impl ConfigurationBank {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Bank0 => 0,
            Self::Bank1 => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationMode {
    Mode0,
    Mode1,
    Mode2,
    Mode3,
}
impl ConfigurationMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Mode0 => 0,
            Self::Mode1 => 1,
            Self::Mode2 => 2,
            Self::Mode3 => 3,
        }
    }
}

use super::{ArgumentKind, CommandError};
/// Duration encoded by the firmware as a byte of minutes and 0..=60 seconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdjustmentDuration(u16);
impl AdjustmentDuration {
    pub fn new(value: u16) -> Result<Self, CommandError> {
        if (1..=15360).contains(&value) {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::AdjustmentDuration))
        }
    }
    pub fn value(self) -> u16 {
        self.0
    }
    pub(super) fn wire(self) -> (u8, u8) {
        let minutes = (self.0 / 60).min(255);
        (minutes as u8, (self.0 - minutes * 60) as u8)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentProfile {
    Profile0,
    Profile1,
    Profile2,
    Profile3,
    Profile4,
    Profile5,
    Profile6,
}
impl AdjustmentProfile {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Profile0 => 0,
            Self::Profile1 => 1,
            Self::Profile2 => 2,
            Self::Profile3 => 3,
            Self::Profile4 => 4,
            Self::Profile5 => 5,
            Self::Profile6 => 6,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentConfigurationAction {
    ReadStatus,
    Apply {
        profile: AdjustmentProfile,
        duration: AdjustmentDuration,
    },
}
/// Internal service flags, not shooting or camera UI modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentMode {
    Mode8,
    Mode16,
}
impl AdjustmentMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Mode8 => 1,
            Self::Mode16 => 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdjustmentConfigurationState {
    NotConfigured,
    Configured,
    Unknown(u8),
}
impl AdjustmentConfigurationState {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::NotConfigured,
            1 => Self::Configured,
            n => Self::Unknown(n),
        }
    }
}
