//! Single radio/network service requests. Stored credentials, connection
//! workflows and result polling are separate operations.
use super::{ArgumentKind, CommandError, Toggle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioMode {
    Mode0,
    Mode1,
}
impl RadioMode {
    fn wire(self) -> u8 {
        match self {
            Self::Mode0 => 0,
            Self::Mode1 => 1,
        }
    }
}
/// Firmware channel field (0..=14); no regulatory or connected-device policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioChannel(u8);
impl RadioChannel {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value <= 14 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::RadioChannel))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
/// Seven-bit encoded power field. This is not a dBm value: firmware transforms
/// values above 11 differently in mode 1, and preserves the raw byte in mode 0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioPower(u8);
impl RadioPower {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value < 128 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::RadioPower))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioAction {
    Primary {
        mode: RadioMode,
        channel: RadioChannel,
        power: RadioPower,
        option: Toggle,
    },
    Secondary {
        mode: RadioMode,
        channel: RadioChannel,
    },
}
impl RadioAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::Primary {
                mode,
                channel,
                power,
                option,
            } => {
                put(0, mode.wire());
                put(1, channel.value());
                put(2, power.value() | (option.wire() << 7));
                put(3, 0);
            }
            Self::Secondary { mode, channel } => {
                put(0, mode.wire());
                put(1, channel.value());
                put(3, 1);
            }
        }
    }
}
/// An exact six-byte peer address, not text interpreted or normalized by a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerAddress([u8; 6]);
impl PeerAddress {
    pub fn new(bytes: &[u8]) -> Result<Self, CommandError> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::PeerAddressLength))
    }
    pub fn data(&self) -> &[u8; 6] {
        &self.0
    }
}
/// The command returns whether firmware option 0x41 equals zero. The inverse
/// bit is decoded here without inferring a physical drive's presence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Option41State {
    Zero,
    NonZero,
    Unknown(u8),
}
impl Option41State {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            1 => Self::Zero,
            0 => Self::NonZero,
            n => Self::Unknown(n),
        }
    }
}

/// WPS interprets a zero first byte as broadcast and discards the other five.
/// This type represents only exact peers; use `WpsTarget::Broadcast` otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WpsPeerAddress([u8; 6]);
impl WpsPeerAddress {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        let address: [u8; 6] = data
            .try_into()
            .map_err(|_| CommandError::Argument(ArgumentKind::WpsPeerAddress))?;
        if address[0] == 0 {
            return Err(CommandError::Argument(ArgumentKind::WpsPeerAddress));
        }
        Ok(Self(address))
    }
    pub fn data(&self) -> &[u8; 6] {
        &self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WpsTarget {
    Broadcast,
    Peer { address: WpsPeerAddress },
}
/// Single service requests. Starting consumes the camera's stored WPS mode/PIN
/// and can update its credential configuration from a later callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WpsAction {
    Stop,
    Start {
        target: WpsTarget,
    },
    Connect,
    /// Publishes the progress snapshot to the reply and a firmware configuration
    /// key. This is a write, not an observational getter.
    PublishProgress,
}
impl WpsAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::Stop => put(0, 0),
            Self::Start { target } => {
                put(0, 1);
                if let WpsTarget::Peer { address } = target {
                    for (i, &byte) in address.data().iter().enumerate() {
                        put(i + 1, byte);
                    }
                }
                put(7, 255);
            }
            Self::Connect => put(0, 2),
            Self::PublishProgress => {
                put(0, 3);
                put(7, 255);
            }
        }
    }
}
/// A snapshot of a low-byte callback result. `Unknown(255)` can be the initial
/// sentinel; neither it nor a known result is an instruction to the host runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WpsCallbackResult {
    Success,
    Timeout,
    Failed,
    Overlap,
    Unknown(u8),
}
impl WpsCallbackResult {
    pub(super) fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Success,
            1 => Self::Timeout,
            2 => Self::Failed,
            3 => Self::Overlap,
            n => Self::Unknown(n),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WpsPhase {
    Idle,
    PushButton,
    Pin,
    CredentialsReceived,
    Timeout,
    Failed,
    Overlap,
    Error,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}
/// Encoded `progress + 16 * connected`. Preserve other bytes without guessing
/// a state or claiming that asynchronous camera work has finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WpsProgress {
    State {
        phase: WpsPhase,
        connection: ConnectionState,
    },
    Unknown {
        raw_value: u8,
    },
}
impl WpsProgress {
    pub(super) fn decode(raw: u8) -> Self {
        let phase = match raw & 15 {
            0 => WpsPhase::Idle,
            1 => WpsPhase::PushButton,
            2 => WpsPhase::Pin,
            3 => WpsPhase::CredentialsReceived,
            4 => WpsPhase::Timeout,
            5 => WpsPhase::Failed,
            6 => WpsPhase::Overlap,
            7 => WpsPhase::Error,
            _ => return Self::Unknown { raw_value: raw },
        };
        let connection = match raw >> 4 {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connected,
            _ => return Self::Unknown { raw_value: raw },
        };
        Self::State { phase, connection }
    }
}
/// Both accepted selectors invoke the same firmware scan path. No active/passive
/// scanning semantics are inferred from their numeric encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationScanMode {
    Mode0,
    Mode1,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationAction {
    Stop,
    StartStored,
    Scan { mode: StationScanMode },
}
impl StationAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::Stop => {
                put(0, 0);
                put(1, 0);
            }
            Self::StartStored => {
                put(0, 0);
                put(1, 1);
                put(2, 255);
            }
            Self::Scan { mode } => {
                put(0, 1);
                put(
                    1,
                    match mode {
                        StationScanMode::Mode0 => 0,
                        StationScanMode::Mode1 => 1,
                    },
                );
                put(2, 255);
                put(3, 255);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointRadioMode {
    Mode2,
    Mode3,
}
/// Reviewed AP security fields that do not require the legacy-key buffer this
/// service command cannot supply. No passphrase or credential setup is implied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessPointSecurity {
    Option0,
    Option2,
    Option3,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountryBytes([u8; 2]);
impl CountryBytes {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        data.try_into()
            .map(Self)
            .map_err(|_| CommandError::Argument(ArgumentKind::CountryLength))
    }
    pub fn data(&self) -> &[u8; 2] {
        &self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkReconfigureAction {
    StopServer,
    ReadConfiguration,
    Apply {
        number: u8,
        channel: RadioChannel,
        radio: AccessPointRadioMode,
        security: AccessPointSecurity,
        country: CountryBytes,
    },
}
impl NetworkReconfigureAction {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::StopServer => put(0, 0),
            Self::ReadConfiguration => {
                put(0, 1);
                put(2, 255);
                for i in 8..14 {
                    put(i, 255);
                }
            }
            Self::Apply {
                number,
                channel,
                radio,
                security,
                country,
            } => {
                put(0, 1);
                put(1, number);
                put(2, channel.value());
                put(
                    4,
                    match radio {
                        AccessPointRadioMode::Mode2 => 0,
                        AccessPointRadioMode::Mode3 => 1,
                    },
                );
                put(
                    5,
                    match security {
                        AccessPointSecurity::Option0 => 0,
                        AccessPointSecurity::Option2 => 2,
                        AccessPointSecurity::Option3 => 3,
                    },
                );
                put(6, country.0[0]);
                put(7, country.0[1]);
                for i in 8..14 {
                    put(i, 255);
                }
            }
        }
    }
}
