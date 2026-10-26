//! Camera definitions: identity, configuration and service-mode guard facts for
//! one firmware, loaded from caller-selected JSON (schema 1, the
//! `firmware/<model>/<version>/camera.json` tables). Data only: no USB, no
//! execution policy.
use fujimod_catalog_firmware_identity::{Digest, FirmwareIdentity, IdentityError};
use serde::Deserialize;

/// A JSON number or a `0x`-prefixed hexadecimal string.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawNumber {
    Integer(u64),
    Text(String),
}
impl RawNumber {
    fn value(&self) -> Result<u64, CameraError> {
        match self {
            Self::Integer(value) => Ok(*value),
            Self::Text(text) => {
                let digits = text
                    .strip_prefix("0x")
                    .ok_or_else(|| CameraError::Number(text.clone()))?;
                u64::from_str_radix(digits, 16).map_err(|_| CameraError::Number(text.clone()))
            }
        }
    }
    fn u32(&self) -> Result<u32, CameraError> {
        u32::try_from(self.value()?).map_err(|_| CameraError::Range)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("invalid camera definition JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported camera definition schema {0}")]
    Schema(u32),
    #[error("invalid number {0:?}; use an integer or 0x-prefixed hexadecimal")]
    Number(String),
    #[error("a value is outside its address or size range")]
    Range,
    #[error("hexadecimal byte data is malformed")]
    Bytes,
    #[error("the camera definition has no {0} section")]
    Missing(&'static str),
}

/// A byte range in the camera's 32-bit address space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub address: u32,
    pub size: u32,
}
impl Range {
    fn new(address: u32, size: u32) -> Result<Self, CameraError> {
        if size == 0 || u64::from(address) + u64::from(size) > 1 << 32 {
            return Err(CameraError::Range);
        }
        Ok(Self { address, size })
    }
}
/// A stock code or data region that must hash to `sha256` before any write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionCheck {
    pub name: String,
    pub range: Range,
    pub sha256: Digest,
}
/// A word whose live value must equal `value`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateWord {
    pub name: String,
    pub address: u32,
    pub value: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedRange {
    pub name: String,
    pub range: Range,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Configuration {
    pub address: u32,
    pub cached_address: u32,
    pub size: u32,
    pub max_read: u32,
}
/// Service-mode reset guard: the configuration byte that suppresses the stock
/// teardown defaults path, and the stock state that must hold while arming it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Guard {
    pub offset: u32,
    pub value: u8,
    pub aliases: Vec<u32>,
    pub code_checks: Vec<RegionCheck>,
    pub state_words: Vec<StateWord>,
    pub callback_table: (u32, Vec<u8>),
    pub output_state: Range,
    pub payloads: Vec<NamedRange>,
}
/// Debug access: the configuration byte that enables diagnostic USB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Debug {
    pub flag_offset: u32,
}
/// Persistent debug mode: a temporary adapter that calls the stock one-byte
/// configuration range writer through a borrowed diagnostic callback, the
/// cache helpers that make it executable, and the storage it changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugMode {
    pub callback: u32,
    pub stock_callback: u32,
    pub output_fields: u32,
    pub adapter: (u32, Vec<u8>),
    pub cache_clean: u32,
    pub instruction_invalidate: u32,
    pub barrier: u32,
    pub instruction_barrier: u32,
    pub code_checks: Vec<RegionCheck>,
    /// Storage geometry words; `None` entries are not checked.
    pub storage_geometry: (u32, Vec<Option<u32>>),
    pub storage_banks: Vec<Range>,
    pub preferences: Range,
}
#[derive(Clone, Debug)]
pub struct CameraDefinition {
    identity: FirmwareIdentity,
    pub ram_alias: u32,
    pub products: Vec<String>,
    pub configuration: Configuration,
    guard: Option<Guard>,
    debug: Option<Debug>,
    debug_mode: Option<DebugMode>,
}

#[derive(Deserialize)]
struct RawRegion {
    name: String,
    address: RawNumber,
    size: RawNumber,
    sha256: String,
}
#[derive(Deserialize)]
struct RawStateWord {
    name: String,
    address: RawNumber,
    value: RawNumber,
}
#[derive(Deserialize)]
struct RawNamedRange {
    name: String,
    address: RawNumber,
    size: RawNumber,
}
#[derive(Deserialize)]
struct RawBytes {
    address: RawNumber,
    hex: String,
}
#[derive(Deserialize)]
struct RawRange {
    address: RawNumber,
    size: RawNumber,
}
#[derive(Deserialize)]
struct RawGuard {
    offset: RawNumber,
    value: u8,
    aliases: Vec<RawNumber>,
    code_checks: Vec<RawRegion>,
    state_words: Vec<RawStateWord>,
    callback_table: RawBytes,
    output_state: RawRange,
    payloads: Vec<RawNamedRange>,
}
#[derive(Deserialize)]
struct RawConfiguration {
    address: RawNumber,
    cached_address: RawNumber,
    size: RawNumber,
    max_read: RawNumber,
}
#[derive(Deserialize)]
struct RawDebug {
    flag_offset: RawNumber,
}
#[derive(Deserialize)]
struct RawIdentity {
    products: Vec<String>,
}
#[derive(Deserialize)]
struct RawCallback {
    address: RawNumber,
    stock: RawNumber,
}
#[derive(Deserialize)]
struct RawCache {
    clean: RawNumber,
    instruction_invalidate: RawNumber,
    barrier: RawNumber,
    instruction_barrier: RawNumber,
}
#[derive(Deserialize)]
struct RawGeometry {
    address: RawNumber,
    words: Vec<Option<RawNumber>>,
}
#[derive(Deserialize)]
struct RawStorage {
    geometry: RawGeometry,
    banks: Vec<RawRange>,
}
#[derive(Deserialize)]
struct RawDebugMode {
    callback: RawCallback,
    output_fields: RawNumber,
    adapter: RawBytes,
    cache: RawCache,
    code_checks: Vec<RawRegion>,
    storage: RawStorage,
    preferences: RawRange,
}
#[derive(Deserialize)]
struct RawCamera {
    schema: u32,
    firmware_id: String,
    firmware_sha256: String,
    ram_alias: RawNumber,
    identity: RawIdentity,
    configuration: RawConfiguration,
    guard: Option<RawGuard>,
    debug: Option<RawDebug>,
    debug_mode: Option<RawDebugMode>,
}

fn bytes(hex: &str) -> Result<Vec<u8>, CameraError> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CameraError::Bytes);
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).map_err(|_| CameraError::Bytes))
        .collect()
}
fn range(address: &RawNumber, size: &RawNumber) -> Result<Range, CameraError> {
    Range::new(address.u32()?, size.u32()?)
}

impl CameraDefinition {
    /// Loading requires the selected registry identity; it does not identify a
    /// connected camera or authorize any operation on one.
    pub fn from_json(json: &str, expected: &FirmwareIdentity) -> Result<Self, CameraError> {
        let raw: RawCamera = serde_json::from_str(json)?;
        if raw.schema != 1 {
            return Err(CameraError::Schema(raw.schema));
        }
        let identity = FirmwareIdentity::new(raw.firmware_id, &raw.firmware_sha256)?;
        if &identity != expected {
            return Err(IdentityError::FirmwareMismatch.into());
        }
        if raw
            .identity
            .products
            .iter()
            .any(|product| product.trim().is_empty())
        {
            return Err(IdentityError::EmptyIdentity.into());
        }
        let configuration = Configuration {
            address: raw.configuration.address.u32()?,
            cached_address: raw.configuration.cached_address.u32()?,
            size: raw.configuration.size.u32()?,
            max_read: raw.configuration.max_read.u32()?,
        };
        if configuration.size == 0 || configuration.max_read == 0 {
            return Err(CameraError::Range);
        }
        let guard = raw
            .guard
            .map(|guard| -> Result<Guard, CameraError> {
                let offset = guard.offset.u32()?;
                if offset >= configuration.size {
                    return Err(CameraError::Range);
                }
                let output_state = range(&guard.output_state.address, &guard.output_state.size)?;
                Ok(Guard {
                    offset,
                    value: guard.value,
                    aliases: guard
                        .aliases
                        .iter()
                        .map(RawNumber::u32)
                        .collect::<Result<_, _>>()?,
                    code_checks: guard
                        .code_checks
                        .iter()
                        .map(|check| {
                            Ok(RegionCheck {
                                name: check.name.clone(),
                                range: range(&check.address, &check.size)?,
                                sha256: Digest::parse(&check.sha256)?,
                            })
                        })
                        .collect::<Result<_, CameraError>>()?,
                    state_words: guard
                        .state_words
                        .iter()
                        .map(|word| {
                            Ok(StateWord {
                                name: word.name.clone(),
                                address: word.address.u32()?,
                                value: word.value.u32()?,
                            })
                        })
                        .collect::<Result<_, CameraError>>()?,
                    callback_table: (
                        guard.callback_table.address.u32()?,
                        bytes(&guard.callback_table.hex)?,
                    ),
                    output_state,
                    payloads: guard
                        .payloads
                        .iter()
                        .map(|payload| {
                            Ok(NamedRange {
                                name: payload.name.clone(),
                                range: range(&payload.address, &payload.size)?,
                            })
                        })
                        .collect::<Result<_, CameraError>>()?,
                })
            })
            .transpose()?;
        let debug = raw
            .debug
            .map(|debug| -> Result<Debug, CameraError> {
                let flag_offset = debug.flag_offset.u32()?;
                if flag_offset >= configuration.size {
                    return Err(CameraError::Range);
                }
                Ok(Debug { flag_offset })
            })
            .transpose()?;
        let debug_mode = raw
            .debug_mode
            .map(|mode| -> Result<DebugMode, CameraError> {
                Ok(DebugMode {
                    callback: mode.callback.address.u32()?,
                    stock_callback: mode.callback.stock.u32()?,
                    output_fields: mode.output_fields.u32()?,
                    adapter: (mode.adapter.address.u32()?, bytes(&mode.adapter.hex)?),
                    cache_clean: mode.cache.clean.u32()?,
                    instruction_invalidate: mode.cache.instruction_invalidate.u32()?,
                    barrier: mode.cache.barrier.u32()?,
                    instruction_barrier: mode.cache.instruction_barrier.u32()?,
                    code_checks: mode
                        .code_checks
                        .iter()
                        .map(|check| {
                            Ok(RegionCheck {
                                name: check.name.clone(),
                                range: range(&check.address, &check.size)?,
                                sha256: Digest::parse(&check.sha256)?,
                            })
                        })
                        .collect::<Result<_, CameraError>>()?,
                    storage_geometry: (
                        mode.storage.geometry.address.u32()?,
                        mode.storage
                            .geometry
                            .words
                            .iter()
                            .map(|word| word.as_ref().map(RawNumber::u32).transpose())
                            .collect::<Result<_, _>>()?,
                    ),
                    storage_banks: mode
                        .storage
                        .banks
                        .iter()
                        .map(|bank| range(&bank.address, &bank.size))
                        .collect::<Result<_, _>>()?,
                    preferences: range(&mode.preferences.address, &mode.preferences.size)?,
                })
            })
            .transpose()?;
        Ok(Self {
            identity,
            ram_alias: raw.ram_alias.u32()?,
            products: raw.identity.products,
            configuration,
            guard,
            debug,
            debug_mode,
        })
    }
    pub fn identity(&self) -> &FirmwareIdentity {
        &self.identity
    }
    pub fn guard(&self) -> Result<&Guard, CameraError> {
        self.guard.as_ref().ok_or(CameraError::Missing("guard"))
    }
    pub fn debug(&self) -> Result<Debug, CameraError> {
        self.debug.ok_or(CameraError::Missing("debug"))
    }
    pub fn debug_mode(&self) -> Result<&DebugMode, CameraError> {
        self.debug_mode
            .as_ref()
            .ok_or(CameraError::Missing("debug_mode"))
    }
}

#[cfg(test)]
mod tests;
