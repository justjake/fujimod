use crate::ff80::framing::RequestKind;

pub type Result<T> = std::result::Result<T, Error>;

/// Which USB exchange failed, independently of a camera command's own status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferStage {
    Control(RequestKind),
    BulkIn,
    BulkOut,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Proxy(#[from] fujimod_usb_proxy::Error),
    #[error("USB device access failed: {0}")]
    Usb(#[from] nusb::Error),
    #[error("emulator resource {operation:?} failed: {source}")]
    EmulatorResource {
        operation: crate::emulator::ResourceOperation,
        source: crate::emulator::TransferError,
    },
    #[error("emulator resource {0:?} timed out")]
    EmulatorResourceTimeout(crate::emulator::ResourceOperation),
    #[error("emulator completion during {stage:?} reported {actual} bytes for capacity {capacity}")]
    EmulatorCompletion {
        stage: TransferStage,
        capacity: usize,
        actual: usize,
    },
    #[error("USB transfer failed during {stage:?}: {source} ({transferred:?} bytes transferred)")]
    Transfer {
        stage: TransferStage,
        source: nusb::transfer::TransferError,
        transferred: Option<usize>,
        partial: Vec<u8>,
    },
    #[error("USB transfer timed out during {0:?}; camera completion is unknown")]
    Timeout(TransferStage),
    #[error("short transfer during {stage:?}: expected {expected}, received {actual}")]
    ShortTransfer {
        stage: TransferStage,
        expected: usize,
        actual: usize,
    },
    #[error("the previous exchange did not complete; this session cannot issue more requests")]
    CompletionUnknown,
    #[error("transfer timeout must be nonzero")]
    InvalidTimeout,
    #[error("invalid USB interface selection")]
    InvalidInterface,
    #[error("selected interface must have exactly one bulk IN and one bulk OUT endpoint")]
    AmbiguousEndpoints,
    #[error("invalid protocol response: {0:?}")]
    Protocol(ProtocolError),
    #[error("camera command {command:#x} failed with status {primary:#x}/{secondary:#x}")]
    DeviceStatus {
        command: u32,
        primary: u32,
        secondary: u32,
    },
    #[error("request length {requested} exceeds the supported limit {limit}")]
    LengthLimit { requested: u32, limit: u32 },
    #[error("memory range crosses the 32-bit address space")]
    AddressOverflow,
    #[error("requested memory range is outside the selected data window")]
    OutsideDataWindow,
    #[error("length must be nonzero and representable in the protocol")]
    InvalidLength,
    #[error("argument must be a bounded ASCII token")]
    InvalidToken,
    #[error("USB replay differs at exchange {position}")]
    ReplayMismatch {
        position: usize,
        expected: Option<Box<crate::testing::Exchange>>,
        actual: Box<crate::testing::Exchange>,
    },
    #[error("virtual USB peer failed: {0}")]
    TestPeer(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("bulk read failed after {data_len} bytes: {source}")]
    PartialRead {
        data_len: usize,
        data: Vec<u8>,
        source: Box<Error>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    ExpectationMismatch,
    ReplyLength { expected: usize, actual: usize },
    CommandEcho { expected: u32, actual: u32 },
    FunctionEcho { expected: u32, actual: u32 },
    ParameterEcho,
    ReservedField(u32),
    ZeroPacketLimit,
    InvalidUtf16,
    UnterminatedString,
    DataLength { available: usize, declared: usize },
}

impl From<ProtocolError> for Error {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

pub fn check_limit(requested: u32, limit: u32) -> Result<()> {
    if requested > limit {
        Err(Error::LengthLimit { requested, limit })
    } else {
        Ok(())
    }
}
