//! Firmware-selected App80 codecs. No USB carrier or model defaults.
use super::{Address, CommandCode, DecodeError, Reply, Request};
use fujimod_catalog_service::{Codec, Definition, Entry, ServiceModes};
mod scan;
pub use scan::ScanSelection;
mod pipeline;
pub use pipeline::{
    PipelineAction, PipelineDispatchOption, PipelineDispatchSelector, PipelinePreparationMode,
    PipelineRequestMode, PipelineValue,
};
mod exposure;
pub use exposure::{
    AdjustmentPacketAction, AdjustmentPacketPayload2, AdjustmentPacketPayload3,
    AdjustmentPacketPayload8, AdjustmentPacketRead, AdjustmentPacketReadData,
    AdjustmentPacketWrite, ExposureDuration, ExposureModeOption, ExposureOption, ExposureReadiness,
    ExposureStatusOption,
};
mod samples;
pub use samples::AdjustmentControlAction;
mod device9;
pub use device9::{Device9Action, Device9Count, Device9Payload};
mod adjustment;
pub use adjustment::{AdjustmentOptionByte, AdjustmentOptionStatus};
mod configuration;
pub use configuration::{
    AdjustmentConfigurationAction, AdjustmentConfigurationState, AdjustmentDuration,
    AdjustmentMode, AdjustmentProfile, ConfigurationBank, ConfigurationMode,
};
mod lens;
pub use lens::{
    LensCompletionParameterFlag, LensPositionRange, LensSelectionIndex, LensSelectionRange,
    LensTarget,
};
mod media;
pub use media::{
    AudioCount, AudioPlayback, AudioResult, AudioRoute, ChargeAction, ChargePresence, ChargeQuery,
    ChargeResult, DisplayMode, DisplayOptionAction, HdmiAction, HdmiPacket, HdmiValue,
    NandReadResult, NandSpare, ReportedChargeState, SoundAction, SoundClip, SoundFormat,
    SoundOperation, SoundTrack, StorageErase, StorageEraseResult, StorageStatus, Toggle,
};
mod network;
pub use network::{
    AccessPointRadioMode, AccessPointSecurity, ConnectionState, CountryBytes,
    NetworkReconfigureAction, Option41State, PeerAddress, RadioAction, RadioChannel, RadioMode,
    RadioPower, StationAction, StationScanMode, WpsAction, WpsCallbackResult, WpsPeerAddress,
    WpsPhase, WpsProgress, WpsTarget,
};
mod transfer;
pub use transfer::{
    TransferChannel, TransferHalfwordAddress, TransferLoadMode, TransferOperation, TransferOption,
    TransferPairFirst, TransferPairResult, TransferPayloadByte, TransferPrepareResult,
    TransferResult, TransferSelectedWordKind, TransferWord7Kind, TransferWord16Kind,
    TransferWordAddress, TransferWordReadResult,
};
mod peripheral;
pub use peripheral::{
    AccumulatorAction, BatteryAction, DisplayPatternAction, I2cAction, I2cLength, I2cPayload,
    I2cPort, RegisterAction,
};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("the selected firmware/modes do not define {0:?}")]
    Unsupported(Codec),
    #[error("multiple command identifiers implement {0:?} in the selected modes")]
    Ambiguous(Codec),
    #[error("command {0:?} is not enabled in the selected service modes")]
    Unavailable(CommandCode),
    #[error("{0} is outside its supported argument range")]
    Argument(ArgumentKind),
    #[error("research arguments must fit the 32-byte App80 mailbox")]
    OversizeArguments,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgumentKind {
    PinBank,
    ProfileIndex,
    WordAlignment,
    WordAddress,
    I2cLength,
    SoundClip,
    HdmiValue,
    HdmiPacketLength,
    AudioCount,
    RadioChannel,
    RadioPower,
    PeerAddressLength,
    WpsPeerAddress,
    CountryLength,
    TransferWordAddress,
    TransferPairFirst,
    AdjustmentDuration,
    LensSelectionIndex,
    Device9Count,
    AdjustmentPacketLength,
    PipelineValue,
    ScanSelection,
}
impl std::fmt::Display for ArgumentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ScanSelection => "scan index below the observed result count",
            Self::PipelineValue => "pipeline request value (1..=255)",
            Self::AdjustmentPacketLength => "adjustment packet payload length",
            Self::Device9Count => "device-9 payload length (0..=29 bytes)",
            Self::PinBank => "pin bank",
            Self::ProfileIndex => "profile index",
            Self::WordAlignment => "word alignment",
            Self::WordAddress => "service word address",
            Self::I2cLength => "I2C payload length (1..=10 bytes)",
            Self::SoundClip => "sound clip index (0..=22)",
            Self::HdmiValue => "HDMI value (0..=4095)",
            Self::HdmiPacketLength => "HDMI packet length (16 bytes)",
            Self::AudioCount => "audio chunk count (0..=10)",
            Self::RadioChannel => "radio channel field (0..=14)",
            Self::RadioPower => "encoded radio power field (0..=127)",
            Self::PeerAddressLength => "peer address length (6 bytes)",
            Self::WpsPeerAddress => "exact WPS peer (6 bytes, first byte nonzero)",
            Self::CountryLength => "country field length (2 bytes)",
            Self::TransferWordAddress => "transfer word address (aligned, below 64 KiB)",
            Self::TransferPairFirst => "first transfer pair field (0..=511)",
            Self::AdjustmentDuration => "adjustment duration (1..=15360 seconds)",
            Self::LensSelectionIndex => "lens selection index (0..=254, excluding sentinel 160)",
        })
    }
}
fn checked_nibble(value: u8, kind: ArgumentKind) -> Result<u8, CommandError> {
    if value < 16 {
        Ok(value)
    } else {
        Err(CommandError::Argument(kind))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PinBank(u8);
impl PinBank {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        checked_nibble(value, ArgumentKind::PinBank).map(Self)
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileIndex(u8);
impl ProfileIndex {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        checked_nibble(value, ArgumentKind::ProfileIndex).map(Self)
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileBank {
    First,
    Second,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileAction {
    Read {
        bank: ProfileBank,
        index: ProfileIndex,
    },
    Write {
        bank: ProfileBank,
        index: ProfileIndex,
        value: u8,
    },
    Reset,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WordAddress(Address);
impl WordAddress {
    pub fn new(value: Address) -> Result<Self, CommandError> {
        if !value.0.is_multiple_of(4) {
            return Err(CommandError::Argument(ArgumentKind::WordAlignment));
        }
        Ok(Self(value))
    }
    pub fn address(self) -> Address {
        self.0
    }
}
/// Selector choices are variants. Integers represent quantities, not commands
/// or completion instructions; command bytes come only from the definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    WritePins {
        bank: PinBank,
        levels: u16,
        exclude: u16,
    },
    ReadPins {
        bank: PinBank,
    },
    ReadChannel {
        channel: u8,
    },
    WriteChannel {
        channel: u8,
        value: u16,
    },
    ProfileByte {
        action: ProfileAction,
    },
    DownloadStatus,
    CopyConfiguration {
        page: u8,
    },
    FlushConfiguration,
    ReadWord {
        address: WordAddress,
    },
    WriteWord {
        address: WordAddress,
        value: u32,
    },
    RegisterControl {
        action: RegisterAction,
    },
    I2c {
        action: I2cAction,
    },
    Accumulator {
        action: AccumulatorAction,
    },
    PeripheralReport,
    BatteryControl {
        action: BatteryAction,
    },
    InitializeDisplay,
    DisplayPattern {
        action: DisplayPatternAction,
    },
    DisplayOption {
        action: DisplayOptionAction,
    },
    DisplayMode {
        mode: DisplayMode,
    },
    Sound {
        action: SoundAction,
    },
    Hdmi {
        action: HdmiAction,
    },
    ParameterService {
        enabled: Toggle,
    },
    ChargeControl {
        action: ChargeAction,
    },
    ChargeStatus {
        query: ChargeQuery,
    },
    AudioCapture {
        count: AudioCount,
        route: AudioRoute,
    },
    AudioPlayback {
        mode: AudioPlayback,
    },
    EraseStorage {
        action: StorageErase,
    },
    StorageStatus,
    FormatStorage,
    DisplaySetup,
    NandReadBlocks {
        spare: NandSpare,
        block_group: u8,
    },
    Option41,
    WlanControl {
        enabled: Toggle,
    },
    RadioControl {
        action: RadioAction,
    },
    SignalStrength {
        address: PeerAddress,
    },
    Iperf,
    Wps {
        action: WpsAction,
    },
    Station {
        action: StationAction,
    },
    NetworkReconfigure {
        action: NetworkReconfigureAction,
    },
    TransferPrepare,
    TransferSelectOperation {
        operation: TransferOperation,
    },
    TransferReady,
    TransferSelectChannel {
        channel: TransferChannel,
    },
    TransferStart,
    TransferReadWord {
        address: TransferWordAddress,
    },
    TransferWriteWord {
        address: TransferWordAddress,
        value: u32,
    },
    TransferPair {
        first: TransferPairFirst,
        second: u16,
    },
    TransferBitfields {
        mode: TransferPayloadByte,
        first: u16,
        second: u16,
    },
    TransferByte {
        data: TransferPayloadByte,
    },
    TransferModeByte {
        data: TransferPayloadByte,
    },
    TransferReadHalfword {
        address: TransferHalfwordAddress,
    },
    TransferWriteHalfword {
        address: TransferHalfwordAddress,
        value: u16,
    },
    TransferReadOptionWord {
        option: TransferOption,
    },
    TransferSetSelectedWord {
        kind: TransferSelectedWordKind,
        value: u16,
    },
    TransferSetWord7 {
        kind: TransferWord7Kind,
        value: u16,
    },
    TransferSetWord16 {
        kind: TransferWord16Kind,
        value: u16,
    },
    TransferLoadTest {
        mode: TransferLoadMode,
    },
    /// Submits flags (kind 0x14) before requesting the two raw bank snapshots.
    ReadConfigurationFlags {
        bank: ConfigurationBank,
        option: Toggle,
    },
    ApplyConfigurationFlags {
        bank: ConfigurationBank,
        option: Toggle,
    },
    SelectConfigurationMode {
        flag: Toggle,
        mode: ConfigurationMode,
    },
    PackConfigurationOptions {
        flag: Toggle,
        option: Toggle,
        upper: u8,
        lower: u8,
    },
    ReadConfigurationOptions,
    AdjustmentConfiguration {
        action: AdjustmentConfigurationAction,
    },
    SelectAdjustmentMode {
        mode: AdjustmentMode,
    },
    LensSelect {
        target: LensTarget,
    },
    LensOperation2,
    LensOperation3,
    LensOperation4,
    LensOperation5,
    LensOperation6,
    PublishLensCompletionParameter,
    PrepareAdjustmentOutput,
    PrepareAlternateAdjustmentOutput,
    LensBoundedPosition {
        position: u8,
        value: u8,
    },
    LensFixedPosition,
    InvokePrimaryParameterCallback {
        parameter: u8,
    },
    InvokeSecondaryParameterCallback {
        parameter: u8,
    },
    /// Requests S53C lens positioning; callback delivery does not confirm movement.
    SetLensPosition {
        position: i16,
    },
    RequestLensPosition,
    /// Low 16 bits returned by the parameter notifier, not a validated position.
    ReadLensParameter,
    AdjustmentControlOption {
        action: AdjustmentControlAction,
    },
    AdjustmentCapture83,
    AdjustmentCaptureSequence,
    AdjustmentSelectSamples,
    AdjustmentCapture8a,
    AdjustmentAverageCaptures {
        count: u8,
    },
    AdjustmentQueryPlaneSamples,
    /// Reads one entry using a prior count observation, which may become stale.
    ReadScan {
        selection: ScanSelection,
    },
    ReportServiceSuccess,
    ReportServiceFailure,
    /// Firmware returns without signaling completion; the session may time out.
    ReturnWithoutSignal,
    Pipeline {
        action: PipelineAction,
    },
    ReadAdjustmentStatusOption,
    RequestExposure {
        mode: ExposureModeOption,
        duration: ExposureDuration,
        option: ExposureOption,
    },
    AdjustmentPacket {
        action: AdjustmentPacketAction,
    },
    ReadAdjustmentReadyOption,
    Device9 {
        action: Device9Action,
    },
    QueryAdjustmentOption {
        option: AdjustmentOptionByte,
    },
}
impl Command {
    pub fn codec(self) -> Codec {
        match self {
            Self::WritePins { .. } => Codec::WritePins,
            Self::ReadPins { .. } => Codec::ReadPins,
            Self::ReadChannel { .. } => Codec::ReadChannel,
            Self::WriteChannel { .. } => Codec::WriteChannel,
            Self::ProfileByte { action: _ } => Codec::ProfileByte,
            Self::DownloadStatus => Codec::DownloadStatus,
            Self::CopyConfiguration { .. } => Codec::CopyConfiguration,
            Self::FlushConfiguration => Codec::FlushConfiguration,
            Self::ReadWord { .. } | Self::WriteWord { .. } => Codec::MemoryWord,
            Self::RegisterControl { action: _ } => Codec::RegisterControl,
            Self::I2c { action: _ } => Codec::I2c,
            Self::Accumulator { action: _ } => Codec::Accumulator,
            Self::PeripheralReport => Codec::PeripheralReport,
            Self::BatteryControl { .. } => Codec::BatteryControl,
            Self::InitializeDisplay => Codec::InitializeDisplay,
            Self::DisplayPattern { .. } => Codec::DisplayPattern,
            Self::DisplayOption { action: _ } => Codec::DisplayOption,
            Self::DisplayMode { mode: _ } => Codec::DisplayMode,
            Self::Sound { action: _ } => Codec::Sound,
            Self::Hdmi { action: _ } => Codec::Hdmi,
            Self::ParameterService { enabled: _ } => Codec::ParameterService,
            Self::ChargeControl { action: _ } => Codec::ChargeControl,
            Self::ChargeStatus { query: _ } => Codec::ChargeStatus,
            Self::AudioCapture { .. } => Codec::AudioCapture,
            Self::AudioPlayback { mode: _ } => Codec::AudioPlayback,
            Self::EraseStorage { action: _ } => Codec::EraseStorage,
            Self::StorageStatus => Codec::StorageStatus,
            Self::FormatStorage => Codec::FormatStorage,
            Self::DisplaySetup => Codec::DisplaySetup,
            Self::NandReadBlocks { .. } => Codec::NandReadBlocks,
            Self::Option41 => Codec::Option41,
            Self::WlanControl { enabled: _ } => Codec::WlanControl,
            Self::RadioControl { action: _ } => Codec::RadioControl,
            Self::SignalStrength { address: _ } => Codec::SignalStrength,
            Self::Iperf => Codec::Iperf,
            Self::Wps { .. } => Codec::Wps,
            Self::Station { .. } => Codec::Station,
            Self::NetworkReconfigure { .. } => Codec::NetworkReconfigure,
            Self::TransferPrepare => Codec::TransferPrepare,
            Self::TransferSelectOperation { .. } => Codec::TransferSelectOperation,
            Self::TransferReady => Codec::TransferReady,
            Self::TransferSelectChannel { .. } => Codec::TransferSelectChannel,
            Self::TransferStart => Codec::TransferStart,
            Self::TransferReadWord { .. } | Self::TransferWriteWord { .. } => Codec::TransferWord,
            Self::TransferPair { .. } => Codec::TransferPair,
            Self::TransferBitfields { .. } => Codec::TransferBitfields,
            Self::TransferByte { .. } => Codec::TransferByte,
            Self::TransferModeByte { .. } => Codec::TransferModeByte,
            Self::TransferReadHalfword { .. } | Self::TransferWriteHalfword { .. } => {
                Codec::TransferHalfword
            }
            Self::TransferReadOptionWord { .. } => Codec::TransferOptionWord,
            Self::TransferSetSelectedWord { .. } => Codec::TransferSelectedWord,
            Self::TransferSetWord7 { .. } => Codec::TransferWord7,
            Self::TransferSetWord16 { .. } => Codec::TransferWord16,
            Self::TransferLoadTest { .. } => Codec::TransferLoadTest,
            Self::ReadConfigurationFlags { .. } | Self::ApplyConfigurationFlags { .. } => {
                Codec::ConfigurationFlags
            }
            Self::SelectConfigurationMode { .. } => Codec::ConfigurationMode,
            Self::PackConfigurationOptions { .. } => Codec::ConfigurationPackOptions,
            Self::ReadConfigurationOptions => Codec::ConfigurationReadOptions,
            Self::AdjustmentConfiguration { .. } => Codec::AdjustmentConfiguration,
            Self::SelectAdjustmentMode { .. } => Codec::SelectAdjustmentMode,
            Self::LensSelect { .. } => Codec::LensSelect,
            Self::LensOperation2 => Codec::LensOperation2,
            Self::LensOperation3 => Codec::LensOperation3,
            Self::LensOperation4 => Codec::LensOperation4,
            Self::LensOperation5 => Codec::LensOperation5,
            Self::LensOperation6 => Codec::LensOperation6,
            Self::PublishLensCompletionParameter => Codec::PublishLensCompletionParameter,
            Self::PrepareAdjustmentOutput => Codec::PrepareAdjustmentOutput,
            Self::PrepareAlternateAdjustmentOutput => Codec::PrepareAlternateAdjustmentOutput,
            Self::LensBoundedPosition { .. } => Codec::LensBoundedPosition,
            Self::LensFixedPosition => Codec::LensFixedPosition,
            Self::InvokePrimaryParameterCallback { .. } => Codec::InvokePrimaryParameterCallback,
            Self::InvokeSecondaryParameterCallback { .. } => {
                Codec::InvokeSecondaryParameterCallback
            }
            Self::AdjustmentControlOption { .. } => Codec::AdjustmentControlOption,
            Self::AdjustmentCapture83 => Codec::AdjustmentCapture83,
            Self::AdjustmentCaptureSequence => Codec::AdjustmentCaptureSequence,
            Self::AdjustmentSelectSamples => Codec::AdjustmentSelectSamples,
            Self::AdjustmentCapture8a => Codec::AdjustmentCapture8a,
            Self::AdjustmentAverageCaptures { .. } => Codec::AdjustmentAverageCaptures,
            Self::AdjustmentQueryPlaneSamples => Codec::AdjustmentQueryPlaneSamples,
            Self::ReadScan { .. } => Codec::ReadScan,
            Self::ReportServiceSuccess => Codec::ReportServiceSuccess,
            Self::ReportServiceFailure => Codec::ReportServiceFailure,
            Self::ReturnWithoutSignal => Codec::ReturnWithoutSignal,
            Self::Pipeline { action } => action.codec(),
            Self::ReadAdjustmentStatusOption => Codec::AdjustmentStatusOption,
            Self::RequestExposure { .. } => Codec::AdjustmentExposure,
            Self::AdjustmentPacket { .. } => Codec::AdjustmentPacket,
            Self::ReadAdjustmentReadyOption => Codec::AdjustmentReadyOption,
            Self::Device9 { .. } => Codec::Device9Exchange,
            Self::QueryAdjustmentOption { .. } => Codec::QueryAdjustmentOption,
            Self::SetLensPosition { .. } => Codec::SetLensPosition,
            Self::RequestLensPosition => Codec::RequestLensPosition,
            Self::ReadLensParameter => Codec::ReadLensParameter,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveStatus {
    Success,
    Failed,
    Unknown(u8),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordReadResult {
    Value { value: u32 },
    Rejected,
    Unknown(u8),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordWriteResult {
    Confirmed,
    Rejected,
    Unknown(u8),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplyValue {
    NoPayload,
    Pins {
        levels: u16,
    },
    Channel {
        value: u16,
    },
    ProfileByte {
        value: u8,
    },
    DownloadStatus {
        raw_status: u8,
    },
    ConfigurationSaved {
        status: SaveStatus,
    },
    ReadWord {
        result: WordReadResult,
    },
    WriteWord {
        result: WordWriteResult,
    },
    Research,
    I2cData {
        data: Vec<u8>,
    },
    Accumulated {
        values: [u16; 3],
    },
    PeripheralReport {
        data: [u8; 8],
    },
    ChargeApplied {
        status: ChargeResult,
    },
    ChargePresence {
        status: ChargePresence,
    },
    ChargeState {
        status: ReportedChargeState,
    },
    AudioCaptured {
        status: AudioResult,
    },
    AudioPlayed {
        status: AudioResult,
    },
    StorageErased {
        status: StorageEraseResult,
    },
    StorageStatus {
        status: StorageStatus,
    },
    NandRead {
        status: NandReadResult,
    },
    Option41 {
        state: Option41State,
    },
    SignalStrength {
        raw_value: u8,
    },
    Iperf {
        raw_result: u8,
    },
    WpsCallback {
        snapshot: WpsCallbackResult,
    },
    WpsProgress {
        snapshot: WpsProgress,
    },
    StationConnection {
        raw_result: u8,
    },
    StationScan {
        raw_result: u8,
        raw_count: u8,
    },
    NetworkConfiguration {
        data: [u8; 6],
    },
    TransferPrepared {
        result: TransferPrepareResult,
    },
    TransferOperationSelected {
        result: TransferResult,
    },
    TransferReadiness {
        result: TransferResult,
    },
    TransferChannelSelected {
        result: TransferResult,
    },
    TransferStarted {
        result: TransferResult,
    },
    TransferWordRead {
        result: TransferWordReadResult,
    },
    TransferWordWritten {
        result: TransferResult,
    },
    TransferPair {
        result: TransferPairResult,
    },
    TransferBytes {
        data: [u8; 2],
    },
    TransferHalfword {
        raw_value: u16,
    },
    TransferOptionWord {
        raw_value: u32,
    },
    /// Two four-byte snapshots in published slot order (1,0,3,2 per buffer).
    ConfigurationFlags {
        raw_data: [u8; 8],
    },
    ConfigurationOptions {
        raw_value: u32,
    },
    AdjustmentConfiguration {
        state: AdjustmentConfigurationState,
    },
    LensSelection {
        range: LensSelectionRange,
        raw_detail: [u8; 2],
    },
    LensOperation {
        raw_value: u8,
    },
    LensParameter {
        raw_value: i16,
    },
    ScanName {
        data: [u8; 32],
    },
    Device9Data {
        data: Vec<u8>,
    },
    AdjustmentStatusOption {
        value: ExposureStatusOption,
    },
    AdjustmentReadyOption {
        value: ExposureReadiness,
    },
    AdjustmentPacketRead {
        data: AdjustmentPacketReadData,
    },
    AdjustmentControlOption {
        raw_value: u8,
    },
    LensCompletionParameter {
        flag: LensCompletionParameterFlag,
        encoded_value: u8,
    },
    LensPosition {
        range: LensPositionRange,
    },
    AdjustmentCallback {
        raw_value: u8,
    },
    AdjustmentOption {
        status: AdjustmentOptionStatus,
        raw_value: u16,
    },
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandReply {
    value: ReplyValue,
    raw: Reply,
}
impl CommandReply {
    pub fn value(&self) -> &ReplyValue {
        &self.value
    }
    pub fn raw(&self) -> &Reply {
        &self.raw
    }
}
#[derive(Clone, Debug)]
enum Decode {
    Typed(Command),
    Research,
}
#[derive(Clone, Debug)]
pub struct PreparedCommand {
    request: Request,
    slots: Vec<u8>,
    decode: Decode,
}
impl PreparedCommand {
    pub fn request(&self) -> &Request {
        &self.request
    }
    pub fn decode_bytes(&self, bytes: &[u8]) -> Result<CommandReply, DecodeError> {
        self.decode(Reply::decode_length(bytes, self.request.encoded_len())?)
    }
    pub fn decode(&self, reply: Reply) -> Result<CommandReply, DecodeError> {
        if reply.raw().len() != self.request.encoded_len() {
            return Err(DecodeError::ReplyLength {
                expected: self.request.encoded_len(),
                actual: reply.raw().len(),
            });
        }
        let Decode::Typed(command) = self.decode else {
            return Ok(CommandReply {
                value: ReplyValue::Research,
                raw: reply,
            });
        };
        // These reviewed codecs preserve the published command byte. Research
        // replies remain opaque, including firmware that changes that byte.
        if reply.raw()[0] != self.request.code().0 {
            return Err(DecodeError::CommandEcho {
                expected: self.request.code().0,
                actual: reply.raw()[0],
            });
        }
        let byte = |index: usize| reply.raw()[1 + usize::from(self.slots[index])];
        let half = |a, b| u16::from_be_bytes([byte(a), byte(b)]);
        let word = || u32::from_be_bytes([byte(6), byte(7), byte(8), byte(9)]);
        let value = match command {
            Command::ReadPins { .. } => ReplyValue::Pins { levels: half(1, 2) },
            Command::ReadChannel { .. } => ReplyValue::Channel { value: half(1, 2) },
            Command::ProfileByte {
                action: ProfileAction::Read { .. },
            } => ReplyValue::ProfileByte { value: byte(2) },
            Command::DownloadStatus => ReplyValue::DownloadStatus {
                raw_status: byte(0),
            },
            Command::FlushConfiguration => ReplyValue::ConfigurationSaved {
                status: match byte(0) {
                    0 => SaveStatus::Success,
                    1 => SaveStatus::Failed,
                    n => SaveStatus::Unknown(n),
                },
            },
            Command::ReadWord { .. } => ReplyValue::ReadWord {
                result: match byte(1) {
                    1 => WordReadResult::Value { value: word() },
                    0 => WordReadResult::Rejected,
                    n => WordReadResult::Unknown(n),
                },
            },
            Command::WriteWord { .. } => ReplyValue::WriteWord {
                result: match byte(1) {
                    1 => WordWriteResult::Confirmed,
                    0 => WordWriteResult::Rejected,
                    n => WordWriteResult::Unknown(n),
                },
            },
            Command::I2c {
                action: I2cAction::Read { length, .. },
            } => ReplyValue::I2cData {
                data: (0..usize::from(length.value()))
                    .map(|index| byte(4 + index))
                    .collect(),
            },
            Command::Accumulator {
                action: AccumulatorAction::StopAndRead,
            } => ReplyValue::Accumulated {
                values: [half(1, 2), half(3, 4), half(5, 6)],
            },
            Command::PeripheralReport => ReplyValue::PeripheralReport {
                data: std::array::from_fn(byte),
            },
            Command::ChargeControl { action: _ } => ReplyValue::ChargeApplied {
                status: ChargeResult::decode(byte(2)),
            },
            Command::ChargeStatus {
                query: ChargeQuery::Presence,
            } => ReplyValue::ChargePresence {
                status: ChargePresence::decode(byte(1)),
            },
            Command::ChargeStatus {
                query: ChargeQuery::State,
            } => ReplyValue::ChargeState {
                status: ReportedChargeState::decode(byte(1)),
            },
            Command::AudioCapture { .. } => ReplyValue::AudioCaptured {
                status: AudioResult::decode(byte(2)),
            },
            Command::AudioPlayback { mode: _ } => ReplyValue::AudioPlayed {
                status: AudioResult::decode(byte(1)),
            },
            Command::EraseStorage { action: _ } => ReplyValue::StorageErased {
                status: StorageEraseResult::decode(byte(1)),
            },
            Command::StorageStatus => ReplyValue::StorageStatus {
                status: StorageStatus::decode(byte(0)),
            },
            Command::NandReadBlocks { .. } => ReplyValue::NandRead {
                status: NandReadResult::decode(byte(2)),
            },
            Command::Option41 => ReplyValue::Option41 {
                state: Option41State::decode(byte(0)),
            },
            Command::SignalStrength { address: _ } => {
                ReplyValue::SignalStrength { raw_value: byte(6) }
            }
            Command::Iperf => ReplyValue::Iperf {
                raw_result: byte(0),
            },
            Command::Wps {
                action: WpsAction::Start { .. },
            } => ReplyValue::WpsCallback {
                snapshot: WpsCallbackResult::decode(byte(7)),
            },
            Command::Wps {
                action: WpsAction::PublishProgress,
            } => ReplyValue::WpsProgress {
                snapshot: WpsProgress::decode(byte(7)),
            },
            Command::Station {
                action: StationAction::StartStored,
            } => ReplyValue::StationConnection {
                raw_result: byte(2),
            },
            Command::Station {
                action: StationAction::Scan { .. },
            } => ReplyValue::StationScan {
                raw_result: byte(2),
                raw_count: byte(3),
            },
            Command::NetworkReconfigure {
                action:
                    NetworkReconfigureAction::ReadConfiguration | NetworkReconfigureAction::Apply { .. },
            } => ReplyValue::NetworkConfiguration {
                data: std::array::from_fn(|i| byte(i + 8)),
            },
            Command::TransferPrepare => ReplyValue::TransferPrepared {
                result: TransferPrepareResult::decode(byte(0)),
            },
            Command::TransferSelectOperation { .. } => ReplyValue::TransferOperationSelected {
                result: TransferResult::decode(byte(1)),
            },
            Command::TransferReady => ReplyValue::TransferReadiness {
                result: TransferResult::decode(byte(0)),
            },
            Command::TransferSelectChannel { .. } => ReplyValue::TransferChannelSelected {
                result: TransferResult::decode(byte(1)),
            },
            Command::TransferStart => ReplyValue::TransferStarted {
                result: TransferResult::decode(byte(0)),
            },
            Command::TransferReadWord { .. } => ReplyValue::TransferWordRead {
                result: match byte(9) {
                    0 => TransferWordReadResult::Value {
                        raw_value: u32::from_be_bytes([byte(5), byte(6), byte(7), byte(8)]),
                    },
                    1 => TransferWordReadResult::Rejected,
                    n => TransferWordReadResult::Unknown { raw_status: n },
                },
            },
            Command::TransferWriteWord { .. } => ReplyValue::TransferWordWritten {
                result: TransferResult::decode(byte(9)),
            },
            Command::TransferPair { .. } => ReplyValue::TransferPair {
                result: match byte(6) {
                    0 => TransferPairResult::Values {
                        first: byte(4),
                        second: byte(5),
                    },
                    3 => TransferPairResult::CallbackError,
                    4 => TransferPairResult::Rejected,
                    6 => TransferPairResult::WaitError,
                    n => TransferPairResult::Unknown { raw_status: n },
                },
            },
            Command::TransferByte { .. } | Command::TransferModeByte { .. } => {
                ReplyValue::TransferBytes {
                    data: [byte(1), byte(2)],
                }
            }
            Command::TransferReadHalfword { .. } => ReplyValue::TransferHalfword {
                raw_value: half(5, 6),
            },
            Command::TransferReadOptionWord { .. } => ReplyValue::TransferOptionWord {
                raw_value: u32::from_be_bytes([byte(2), byte(3), byte(4), byte(5)]),
            },
            Command::ReadConfigurationFlags { .. } => ReplyValue::ConfigurationFlags {
                raw_data: [
                    byte(3),
                    byte(4),
                    byte(5),
                    byte(6),
                    byte(7),
                    byte(8),
                    byte(9),
                    byte(10),
                ],
            },
            Command::ReadConfigurationOptions => ReplyValue::ConfigurationOptions {
                raw_value: u32::from_be_bytes([byte(0), byte(1), byte(2), byte(3)]),
            },
            Command::AdjustmentConfiguration {
                action: AdjustmentConfigurationAction::ReadStatus,
            } => ReplyValue::AdjustmentConfiguration {
                state: AdjustmentConfigurationState::decode(byte(4)),
            },
            Command::LensSelect { .. } => ReplyValue::LensSelection {
                range: LensSelectionRange::decode(byte(2)),
                raw_detail: [byte(3), byte(4)],
            },
            Command::LensOperation2 | Command::LensOperation3 | Command::LensFixedPosition => {
                ReplyValue::LensOperation { raw_value: byte(0) }
            }
            Command::PublishLensCompletionParameter => ReplyValue::LensCompletionParameter {
                flag: LensCompletionParameterFlag::decode(byte(0)),
                encoded_value: byte(1),
            },
            Command::LensBoundedPosition { .. } => ReplyValue::LensPosition {
                range: LensPositionRange::decode(byte(2)),
            },
            Command::InvokePrimaryParameterCallback { .. }
            | Command::InvokeSecondaryParameterCallback { .. } => {
                ReplyValue::AdjustmentCallback { raw_value: byte(1) }
            }
            Command::ReadScan { .. } => ReplyValue::ScanName {
                data: std::array::from_fn(|i| byte(i + 1)),
            },
            Command::ReportServiceSuccess
            | Command::ReportServiceFailure
            | Command::ReturnWithoutSignal => ReplyValue::NoPayload,
            Command::Pipeline { .. } => ReplyValue::NoPayload,
            Command::ReadAdjustmentStatusOption => ReplyValue::AdjustmentStatusOption {
                value: ExposureStatusOption::decode(byte(0)),
            },
            Command::ReadAdjustmentReadyOption => ReplyValue::AdjustmentReadyOption {
                value: ExposureReadiness::decode(byte(0)),
            },
            Command::AdjustmentPacket {
                action: AdjustmentPacketAction::Read { kind },
            } => ReplyValue::AdjustmentPacketRead {
                data: match kind {
                    AdjustmentPacketRead::Kind20 => AdjustmentPacketReadData::Kind20 {
                        data: [
                            byte(1),
                            byte(2),
                            byte(3),
                            byte(4),
                            byte(5),
                            byte(6),
                            byte(7),
                        ],
                    },
                    AdjustmentPacketRead::Kind21 => {
                        AdjustmentPacketReadData::Kind21 { value: byte(1) }
                    }
                    AdjustmentPacketRead::Kind22 => AdjustmentPacketReadData::Kind22 {
                        data: [byte(1), byte(2), byte(3)],
                    },
                    AdjustmentPacketRead::Kind23 => {
                        AdjustmentPacketReadData::Kind23 { value: byte(1) }
                    }
                },
            },
            Command::RequestExposure { .. }
            | Command::AdjustmentPacket {
                action: AdjustmentPacketAction::Write { .. },
            } => ReplyValue::NoPayload,
            Command::AdjustmentControlOption {
                action: AdjustmentControlAction::Read { .. },
            } => ReplyValue::AdjustmentControlOption { raw_value: byte(3) },
            Command::AdjustmentControlOption {
                action: AdjustmentControlAction::Write { .. },
            }
            | Command::AdjustmentCapture83
            | Command::AdjustmentCaptureSequence
            | Command::AdjustmentSelectSamples
            | Command::AdjustmentCapture8a
            | Command::AdjustmentAverageCaptures { .. }
            | Command::AdjustmentQueryPlaneSamples => ReplyValue::NoPayload,
            Command::Device9 {
                action: Device9Action::Read { count, .. },
            } => ReplyValue::Device9Data {
                data: (0..usize::from(count.value()))
                    .map(|i| byte(4 + i))
                    .collect(),
            },
            Command::Device9 {
                action: Device9Action::Write { .. },
            } => ReplyValue::NoPayload,
            Command::ReadLensParameter => ReplyValue::LensParameter {
                raw_value: i16::from_be_bytes([byte(0), byte(1)]),
            },
            Command::QueryAdjustmentOption { .. } => ReplyValue::AdjustmentOption {
                status: AdjustmentOptionStatus::decode(byte(3)),
                raw_value: half(1, 2),
            },
            Command::SetLensPosition { .. }
            | Command::RequestLensPosition
            | Command::LensOperation6
            | Command::PrepareAdjustmentOutput
            | Command::PrepareAlternateAdjustmentOutput
            | Command::AdjustmentConfiguration {
                action: AdjustmentConfigurationAction::Apply { .. },
            }
            | Command::SelectAdjustmentMode { .. }
            | Command::LensOperation4
            | Command::LensOperation5
            | Command::BatteryControl { .. }
            | Command::InitializeDisplay
            | Command::DisplayPattern { .. }
            | Command::ApplyConfigurationFlags { .. }
            | Command::SelectConfigurationMode { .. }
            | Command::PackConfigurationOptions { .. }
            | Command::TransferBitfields { .. }
            | Command::TransferWriteHalfword { .. }
            | Command::TransferSetSelectedWord { .. }
            | Command::TransferSetWord7 { .. }
            | Command::TransferSetWord16 { .. }
            | Command::TransferLoadTest { .. }
            | Command::NetworkReconfigure {
                action: NetworkReconfigureAction::StopServer,
            }
            | Command::Wps {
                action: WpsAction::Stop | WpsAction::Connect,
            }
            | Command::Station {
                action: StationAction::Stop,
            }
            | Command::WlanControl {
                enabled: Toggle::Disabled | Toggle::Enabled,
            }
            | Command::RadioControl {
                action: RadioAction::Primary { .. } | RadioAction::Secondary { .. },
            }
            | Command::FormatStorage
            | Command::DisplaySetup => ReplyValue::NoPayload,
            Command::WritePins { .. }
            | Command::WriteChannel { .. }
            | Command::ProfileByte {
                action: ProfileAction::Write { .. } | ProfileAction::Reset,
            }
            | Command::CopyConfiguration { .. }
            | Command::RegisterControl {
                action:
                    RegisterAction::AudioValue { .. }
                    | RegisterAction::DisplayFirst { .. }
                    | RegisterAction::DisplayStub { .. }
                    | RegisterAction::DisplaySecond { .. }
                    | RegisterAction::PowerWrite { .. }
                    | RegisterAction::PositionPair { .. }
                    | RegisterAction::NoopFirst
                    | RegisterAction::NoopSecond,
            }
            | Command::I2c {
                action: I2cAction::Write { .. } | I2cAction::WriteSingle { .. },
            }
            | Command::Accumulator {
                action: AccumulatorAction::Start,
            }
            | Command::DisplayOption {
                action: DisplayOptionAction::IdlePins | DisplayOptionAction::ForwardPair { .. },
            }
            | Command::DisplayMode {
                mode: DisplayMode::Option0 | DisplayMode::Option1,
            }
            | Command::Sound {
                action: SoundAction::Clip { .. } | SoundAction::Track { .. } | SoundAction::Noop,
            }
            | Command::Hdmi {
                action:
                    HdmiAction::ResetStub
                    | HdmiAction::Packet { packet: _ }
                    | HdmiAction::Triplet { .. },
            }
            | Command::ParameterService {
                enabled: Toggle::Disabled | Toggle::Enabled,
            } => ReplyValue::NoPayload,
        };
        Ok(CommandReply { value, raw: reply })
    }
}
#[derive(Clone, Debug)]
pub struct Catalog {
    definition: Definition,
}
impl Catalog {
    pub fn new(definition: Definition) -> Self {
        Self { definition }
    }
    pub fn definition(&self) -> &Definition {
        &self.definition
    }
    pub fn prepare(
        &self,
        modes: ServiceModes,
        command: Command,
    ) -> Result<PreparedCommand, CommandError> {
        let codec = command.codec();
        // Resolve the same first matching row as firmware's dispatch. A later
        // duplicate with a different codec must never override the active row.
        let mut matches = self.definition.entries().iter().filter(|entry| {
            entry.codec() == codec
                && self
                    .definition
                    .select(entry.code(), modes)
                    .is_some_and(|selected| std::ptr::eq(selected, *entry))
        });
        let entry = matches.next().ok_or(CommandError::Unsupported(codec))?;
        if matches.next().is_some() {
            return Err(CommandError::Ambiguous(codec));
        }
        Self::encode(entry, command)
    }
    /// Select an explicit service identifier while retaining typed argument validation.
    /// Useful when firmware gives the same operation several identifiers.
    pub fn prepare_code(
        &self,
        code: CommandCode,
        modes: ServiceModes,
        command: Command,
    ) -> Result<PreparedCommand, CommandError> {
        let entry = self
            .definition
            .select(code.0, modes)
            .ok_or(CommandError::Unavailable(code))?;
        if entry.codec() != command.codec() {
            return Err(CommandError::Unsupported(command.codec()));
        }
        Self::encode(entry, command)
    }
    pub fn prepare_research(
        &self,
        code: CommandCode,
        modes: ServiceModes,
        arguments: &[u8],
    ) -> Result<PreparedCommand, CommandError> {
        self.definition
            .select(code.0, modes)
            .ok_or(CommandError::Unavailable(code))?;
        let request = Request::new(code, arguments).map_err(|_| CommandError::OversizeArguments)?;
        Ok(PreparedCommand {
            request,
            slots: vec![],
            decode: Decode::Research,
        })
    }
    fn encode(entry: &Entry, command: Command) -> Result<PreparedCommand, CommandError> {
        let mut arguments = vec![0; entry.codec().mailbox_arguments()];
        let mut put =
            |index: usize, value: u8| arguments[usize::from(entry.slots()[index])] = value;
        match command {
            Command::WritePins {
                bank,
                levels,
                exclude,
            } => {
                let [hi, lo] = levels.to_be_bytes();
                let [ex_hi, ex_lo] = exclude.to_be_bytes();
                for (i, v) in [bank.value(), hi, lo, ex_hi, ex_lo].into_iter().enumerate() {
                    put(i, v);
                }
            }
            Command::AdjustmentConfiguration { action } => match action {
                AdjustmentConfigurationAction::ReadStatus => {
                    put(0, 0);
                    put(4, 255);
                }
                AdjustmentConfigurationAction::Apply { profile, duration } => {
                    let (minutes, seconds) = duration.wire();
                    put(0, 1);
                    put(1, profile.wire());
                    put(2, minutes);
                    put(3, seconds);
                }
            },
            Command::SelectAdjustmentMode { mode } => put(0, mode.wire()),
            Command::LensSelect { target } => {
                put(0, target.wire());
                // Initialization's extra byte is read but ignored by the lower helper.
                put(1, 0);
                for i in 2..5 {
                    put(i, 255);
                }
            }
            Command::LensOperation2 | Command::LensOperation3 | Command::LensFixedPosition => {
                put(0, 255)
            }
            Command::PublishLensCompletionParameter => {
                put(0, 255);
                put(1, 255);
            }
            Command::LensBoundedPosition { position, value } => {
                put(0, position);
                put(1, value);
                put(2, 255);
            }
            Command::InvokePrimaryParameterCallback { parameter }
            | Command::InvokeSecondaryParameterCallback { parameter } => {
                put(0, parameter);
                put(1, 255);
            }
            Command::ReportServiceSuccess
            | Command::ReportServiceFailure
            | Command::ReturnWithoutSignal => (),
            Command::ReadScan { selection } => put(0, selection.value()),
            Command::Pipeline { action } => action.encode(&mut put),
            Command::ReadAdjustmentStatusOption | Command::ReadAdjustmentReadyOption => put(0, 255),
            Command::RequestExposure {
                mode,
                duration,
                option,
            } => {
                put(0, mode.wire());
                let [high, low] = duration.wire();
                put(1, high);
                put(2, low);
                put(3, option.wire());
            }
            Command::AdjustmentPacket {
                action: AdjustmentPacketAction::Write { packet },
            } => {
                put(0, packet.kind());
                let (bytes, length) = packet.payload();
                for (index, &value) in bytes[..length].iter().enumerate() {
                    put(index + 1, value);
                }
            }
            Command::AdjustmentPacket {
                action: AdjustmentPacketAction::Read { kind },
            } => {
                put(0, kind.kind());
                for index in 1..11 {
                    put(index, 255);
                }
            }
            Command::AdjustmentControlOption { action } => action.encode(&mut put),
            Command::AdjustmentAverageCaptures { count } => put(0, count),
            Command::AdjustmentCapture83
            | Command::AdjustmentCaptureSequence
            | Command::AdjustmentSelectSamples
            | Command::AdjustmentCapture8a
            | Command::AdjustmentQueryPlaneSamples => (),
            Command::Device9 { action } => action.encode(&mut put),
            Command::SetLensPosition { position } => {
                let [high, low] = position.to_be_bytes();
                put(0, high);
                put(1, low);
                put(2, 255);
            }
            Command::RequestLensPosition => put(0, 255),
            Command::ReadLensParameter => {
                put(0, 255);
                put(1, 255);
            }
            Command::QueryAdjustmentOption { option } => {
                put(0, option.value());
                for i in 1..4 {
                    put(i, 255);
                }
            }
            Command::LensOperation6
            | Command::PrepareAdjustmentOutput
            | Command::PrepareAlternateAdjustmentOutput => (),
            Command::LensOperation4 | Command::LensOperation5 => (),
            Command::BatteryControl { action } => action.encode(&mut put),
            Command::InitializeDisplay => put(0, 0),
            Command::DisplayPattern { action } => action.encode(&mut put),
            Command::ReadPins { bank } => put(0, bank.value()),
            Command::ReadChannel { channel } => put(0, channel),
            Command::WriteChannel { channel, value } => {
                let [hi, lo] = value.to_be_bytes();
                for (i, v) in [channel, hi, lo].into_iter().enumerate() {
                    put(i, v);
                }
            }
            Command::ProfileByte { action } => match action {
                ProfileAction::Read { bank, index } => {
                    put(
                        0,
                        match bank {
                            ProfileBank::First => 1,
                            ProfileBank::Second => 2,
                        },
                    );
                    put(1, index.value());
                }
                ProfileAction::Write { bank, index, value } => {
                    put(
                        0,
                        match bank {
                            ProfileBank::First => 3,
                            ProfileBank::Second => 4,
                        },
                    );
                    put(1, index.value());
                    put(2, value);
                }
                ProfileAction::Reset => put(0, 5),
            },
            Command::CopyConfiguration { page } => put(0, page),
            // The handler writes 0 or 1. Start with an unknown sentinel so
            // an unchanged output can never be reported as a successful save.
            Command::FlushConfiguration => put(0, u8::MAX),
            Command::DownloadStatus => (),
            Command::RegisterControl { action } => action.encode(&mut put),
            Command::I2c { action } => action.encode(&mut put),
            Command::Accumulator { action } => action.encode(&mut put),
            Command::PeripheralReport => (),
            Command::DisplayOption { action } => action.encode(&mut put),
            Command::DisplayMode { mode } => put(0, mode.wire()),
            Command::Sound { action } => action.encode(&mut put),
            Command::Hdmi { action } => action.encode(&mut put),
            Command::ParameterService { enabled: value } => put(0, value.wire()),
            Command::ChargeControl { action } => action.encode(&mut put),
            Command::ChargeStatus { query } => query.encode(&mut put),
            Command::AudioCapture { count, route } => {
                put(0, count.value());
                put(1, route.wire());
                put(2, 255);
            }
            Command::AudioPlayback { mode } => {
                put(0, mode.wire());
                put(1, 255);
            }
            Command::EraseStorage { action } => {
                put(0, action.wire());
                put(1, 255);
            }
            Command::StorageStatus => put(0, 255),
            Command::NandReadBlocks { spare, block_group } => {
                put(0, spare.wire());
                put(1, block_group);
                put(2, 255);
            }
            Command::Option41 | Command::Iperf => put(0, 255),
            Command::WlanControl { enabled } => put(0, enabled.wire()),
            Command::RadioControl { action } => action.encode(&mut put),
            Command::Wps { action } => action.encode(&mut put),
            Command::Station { action } => action.encode(&mut put),
            Command::NetworkReconfigure { action } => action.encode(&mut put),
            Command::TransferPrepare | Command::TransferReady | Command::TransferStart => {
                put(0, 255)
            }
            Command::TransferSelectOperation { operation } => {
                put(0, operation.wire());
                put(1, 255);
            }
            Command::TransferSelectChannel { channel } => {
                put(0, channel.wire());
                put(1, 255);
            }
            Command::TransferReadWord { address } | Command::TransferWriteWord { address, .. } => {
                put(
                    0,
                    u8::from(matches!(command, Command::TransferWriteWord { .. })),
                );
                for (i, b) in address.value().to_be_bytes().into_iter().enumerate() {
                    put(i + 1, b);
                }
                if let Command::TransferWriteWord { value, .. } = command {
                    for (i, b) in value.to_be_bytes().into_iter().enumerate() {
                        put(i + 5, b);
                    }
                } else {
                    for i in 5..9 {
                        put(i, 255);
                    }
                }
                put(9, 255);
            }
            Command::TransferPair { first, second } => {
                for (i, b) in first
                    .value()
                    .to_be_bytes()
                    .into_iter()
                    .chain(second.to_be_bytes())
                    .enumerate()
                {
                    put(i, b);
                }
                for i in 4..7 {
                    put(i, 255);
                }
            }
            Command::TransferBitfields {
                mode,
                first,
                second,
            } => {
                put(0, mode.value());
                for (i, b) in first
                    .to_be_bytes()
                    .into_iter()
                    .chain(second.to_be_bytes())
                    .enumerate()
                {
                    put(i + 1, b);
                }
            }
            Command::TransferByte { data } | Command::TransferModeByte { data } => {
                put(0, data.value());
                put(1, 255);
                put(2, 255);
            }
            Command::TransferReadHalfword { address }
            | Command::TransferWriteHalfword { address, .. } => {
                put(
                    0,
                    u8::from(matches!(command, Command::TransferWriteHalfword { .. })),
                );
                for (i, b) in address.value().to_be_bytes().into_iter().enumerate() {
                    put(i + 1, b);
                }
                let [hi, lo] = if let Command::TransferWriteHalfword { value, .. } = command {
                    value.to_be_bytes()
                } else {
                    [255, 255]
                };
                put(5, hi);
                put(6, lo);
            }
            Command::TransferReadOptionWord { option } => {
                let (mode, bank) = option.wire();
                put(0, mode);
                put(1, bank);
                for i in 2..6 {
                    put(i, 255);
                }
            }
            Command::TransferSetSelectedWord { kind, value } => {
                let (selection, mode) = kind.wire();
                put(0, selection);
                put(1, mode);
                let [hi, lo] = value.to_be_bytes();
                put(2, hi);
                put(3, lo);
            }
            Command::TransferSetWord7 { kind, value } => {
                put(0, kind.wire());
                let [hi, lo] = value.to_be_bytes();
                put(1, hi);
                put(2, lo);
            }
            Command::TransferSetWord16 { kind, value } => {
                put(0, kind.wire());
                let [hi, lo] = value.to_be_bytes();
                put(1, hi);
                put(2, lo);
            }
            Command::TransferLoadTest { mode } => put(0, mode.wire()),
            Command::ReadConfigurationFlags { bank, option }
            | Command::ApplyConfigurationFlags { bank, option } => {
                put(
                    0,
                    u8::from(matches!(command, Command::ApplyConfigurationFlags { .. })),
                );
                put(1, bank.wire());
                put(2, option.wire());
                if matches!(command, Command::ReadConfigurationFlags { .. }) {
                    for index in 3..11 {
                        put(index, 255);
                    }
                }
            }
            Command::SelectConfigurationMode { flag, mode } => {
                put(0, flag.wire());
                put(1, mode.wire());
            }
            Command::PackConfigurationOptions {
                flag,
                option,
                upper,
                lower,
            } => {
                put(0, flag.wire());
                put(1, option.wire());
                put(2, upper);
                put(3, lower);
            }
            Command::ReadConfigurationOptions => {
                for index in 0..4 {
                    put(index, 255);
                }
            }
            Command::SignalStrength { address } => {
                for (i, &value) in address.data().iter().enumerate() {
                    put(i, value);
                }
                put(6, 255);
            }
            Command::FormatStorage | Command::DisplaySetup => (),
            Command::ReadWord { address } | Command::WriteWord { address, .. } => {
                let value = address.address().0;
                if value > 0x1fff_ffff && value.wrapping_add(0xc000_0000) > 0x1fff_ffff {
                    return Err(CommandError::Argument(ArgumentKind::WordAddress));
                }
                let write = matches!(command, Command::WriteWord { .. });
                put(0, u8::from(write));
                for (i, v) in value.to_be_bytes().into_iter().enumerate() {
                    put(i + 2, v);
                }
                if let Command::WriteWord { value, .. } = command {
                    for (i, v) in value.to_be_bytes().into_iter().enumerate() {
                        put(i + 6, v);
                    }
                }
            }
        }
        Ok(PreparedCommand {
            request: Request::from_codec(CommandCode(entry.code()), arguments),
            slots: entry.slots().to_vec(),
            decode: Decode::Typed(command),
        })
    }
}

#[cfg(test)]
mod peripheral_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod media_tests;

#[cfg(test)]
mod network_tests;

#[cfg(test)]
mod transfer_tests;

#[cfg(test)]
mod configuration_tests;

#[cfg(test)]
mod lens_tests;

#[cfg(test)]
mod adjustment_tests;

#[cfg(test)]
#[path = "catalog/position_tests.rs"]
mod position_tests;
