//! Generated, independently available service setup and debug persistence facts.
//! Documents contain numeric addresses and bounded data. Human review and
//! execution authorization are caller policy, separate from these contracts.
use fujimod_catalog_firmware_identity::{Digest, FirmwareIdentity, IdentityError};
use serde::{Deserialize, Deserializer};

pub const SCHEMA_URI: &str = "https://fujimod.com/schemas/service-workflow.generated.schema.json";

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("invalid workflow contract: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported workflow contract schema")]
    Schema,
    #[error("invalid workflow address, size or configuration geometry")]
    Range,
    #[error("workflow has no located {0} capability")]
    Missing(&'static str),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub address: u32,
    pub size: u32,
}
impl Range {
    fn validate(&self) -> Result<(), WorkflowError> {
        if self.size == 0 || u64::from(self.address) + u64::from(self.size) > 1 << 32 {
            return Err(WorkflowError::Range);
        }
        Ok(())
    }
}
fn digest<'de, D: Deserializer<'de>>(input: D) -> Result<Digest, D::Error> {
    Digest::parse(&String::deserialize(input)?).map_err(serde::de::Error::custom)
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionCheck {
    pub name: String,
    #[serde(flatten)]
    pub range: Range,
    #[serde(deserialize_with = "digest")]
    pub sha256: Digest,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateWord {
    pub name: String,
    pub address: u32,
    pub value: u32,
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedRange {
    pub name: String,
    #[serde(flatten)]
    pub range: Range,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub address: u32,
    pub cached_address: u32,
    pub size: u32,
    pub page_bytes: u32,
    pub memory_read_bytes: u32,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceIdentity {
    pub products: Vec<String>,
    pub internal_versions: Vec<String>,
    pub code_checks: Vec<RegionCheck>,
}
fn callbacks<'de, D: Deserializer<'de>>(input: D) -> Result<(u32, Vec<u8>), D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Words {
        address: u32,
        words: Vec<u32>,
    }
    let value = Words::deserialize(input)?;
    if value.words.is_empty() || value.words.len() > 32 {
        return Err(serde::de::Error::custom(
            "callback table must contain 1..32 words",
        ));
    }
    Ok((
        value.address,
        value.words.into_iter().flat_map(u32::to_le_bytes).collect(),
    ))
}
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub offset: u32,
    pub value: u8,
    pub aliases: Vec<u32>,
    pub code_checks: Vec<RegionCheck>,
    pub state_words: Vec<StateWord>,
    #[serde(deserialize_with = "callbacks")]
    pub callback_table: (u32, Vec<u8>),
    pub output_state: Range,
    pub payloads: Vec<NamedRange>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceSetup {
    #[serde(skip)]
    pub identity: DeviceIdentity,
    pub ram_alias: u32,
    pub configuration: Configuration,
    pub debug_offset: u32,
    pub guard: Guard,
}
#[derive(Clone, Debug)]
pub struct DebugPersistence {
    pub callback: u32,
    pub stock_callback: u32,
    pub output_fields: u32,
    pub adapter: (u32, Vec<u8>),
    pub cache_clean: u32,
    pub instruction_invalidate: u32,
    pub barrier: u32,
    pub instruction_barrier: u32,
    pub code_checks: Vec<RegionCheck>,
    pub storage_geometry: (u32, Vec<Option<u32>>),
    pub storage_banks: Vec<Range>,
    pub preferences: Range,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Callback {
    address: u32,
    stock: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Adapter {
    address: u32,
    bytes: Vec<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cache {
    clean: u32,
    instruction_invalidate: u32,
    barrier: u32,
    instruction_barrier: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Geometry {
    address: u32,
    words: Vec<Option<u32>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Storage {
    geometry: Geometry,
    banks: Vec<Range>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Persistence {
    callback: Callback,
    output_fields: u32,
    adapter: Adapter,
    cache: Cache,
    code_checks: Vec<RegionCheck>,
    storage: Storage,
    preferences: Range,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    #[serde(rename = "$schema")]
    schema_uri: String,
    schema: u32,
    firmware_id: String,
    firmware_sha256: String,
    identity: DeviceIdentity,
    setup: Option<ServiceSetup>,
    persistence: Option<Persistence>,
}
#[derive(Clone, Debug)]
pub struct WorkflowDefinition {
    identity: FirmwareIdentity,
    setup: Option<ServiceSetup>,
    persistence: Option<DebugPersistence>,
}
impl WorkflowDefinition {
    pub fn from_json(json: &str, expected: &FirmwareIdentity) -> Result<Self, WorkflowError> {
        let mut raw: Document = serde_json::from_str(json)?;
        if raw.schema != 1 || raw.schema_uri != SCHEMA_URI {
            return Err(WorkflowError::Schema);
        }
        let identity = FirmwareIdentity::new(raw.firmware_id, &raw.firmware_sha256)?;
        if &identity != expected {
            return Err(IdentityError::FirmwareMismatch.into());
        }
        if raw.identity.products.is_empty()
            || raw.identity.internal_versions.is_empty()
            || raw
                .identity
                .products
                .iter()
                .chain(&raw.identity.internal_versions)
                .any(|s| s.trim().is_empty())
        {
            return Err(IdentityError::EmptyIdentity.into());
        }
        for check in &raw.identity.code_checks {
            check.range.validate()?;
        }
        if let Some(setup) = &mut raw.setup {
            setup.identity = raw.identity;
            let cfg = setup.configuration;
            Range {
                address: cfg.address,
                size: cfg.size,
            }
            .validate()?;
            Range {
                address: cfg.cached_address,
                size: cfg.size,
            }
            .validate()?;
            if cfg.page_bytes == 0
                || cfg.memory_read_bytes == 0
                || cfg.page_bytes > 256
                || cfg.memory_read_bytes > 256
                || setup.debug_offset >= cfg.size
                || setup.guard.offset >= cfg.size
                || setup.guard.offset == setup.debug_offset
                || setup.guard.offset == 0x80
                || setup.guard.value != 1
            {
                return Err(WorkflowError::Range);
            }
            setup.guard.output_state.validate()?;
            Range {
                address: setup.guard.callback_table.0,
                size: setup.guard.callback_table.1.len() as u32,
            }
            .validate()?;
            for region in &setup.guard.payloads {
                region.range.validate()?;
            }
            for check in &setup.guard.code_checks {
                check.range.validate()?;
            }
        }
        let persistence = raw
            .persistence
            .map(|mode| -> Result<_, WorkflowError> {
                if raw.setup.is_none() {
                    return Err(WorkflowError::Missing("service setup"));
                }
                if mode.adapter.bytes.is_empty()
                    || mode.adapter.bytes.len() > 128
                    || mode.storage.geometry.words.is_empty()
                    || mode.storage.banks.is_empty()
                {
                    return Err(WorkflowError::Range);
                }
                mode.preferences.validate()?;
                Range {
                    address: mode.adapter.address,
                    size: mode.adapter.bytes.len() as u32,
                }
                .validate()?;
                for bank in &mode.storage.banks {
                    bank.validate()?;
                }
                for check in &mode.code_checks {
                    check.range.validate()?;
                }
                Ok(DebugPersistence {
                    callback: mode.callback.address,
                    stock_callback: mode.callback.stock,
                    output_fields: mode.output_fields,
                    adapter: (mode.adapter.address, mode.adapter.bytes),
                    cache_clean: mode.cache.clean,
                    instruction_invalidate: mode.cache.instruction_invalidate,
                    barrier: mode.cache.barrier,
                    instruction_barrier: mode.cache.instruction_barrier,
                    code_checks: mode.code_checks,
                    storage_geometry: (mode.storage.geometry.address, mode.storage.geometry.words),
                    storage_banks: mode.storage.banks,
                    preferences: mode.preferences,
                })
            })
            .transpose()?;
        Ok(Self {
            identity,
            setup: raw.setup,
            persistence,
        })
    }
    pub fn identity(&self) -> &FirmwareIdentity {
        &self.identity
    }
    pub fn setup(&self) -> Result<&ServiceSetup, WorkflowError> {
        self.setup
            .as_ref()
            .ok_or(WorkflowError::Missing("service setup"))
    }
    pub fn persistence(&self) -> Result<&DebugPersistence, WorkflowError> {
        self.persistence
            .as_ref()
            .ok_or(WorkflowError::Missing("debug persistence"))
    }
}
#[cfg(test)]
mod tests;
